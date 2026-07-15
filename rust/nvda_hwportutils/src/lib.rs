//! Rust port of NVDA's hardware-port enumeration (`source/hwPortUtils.py`).
//!
//! Enumerates serial (COM), USB and HID devices via the Win32 SetupAPI, reading
//! per-device registry properties, HID attributes and Bluetooth device info.
//! Pure Rust (no PyO3); `nvda_python` exposes it as `nvdaRust.hwportutils`, and
//! `hwPortUtils.py` delegates to that. Replaces a large amount of error-prone
//! ctypes (SetupAPI's two-call size/data buffer dance, `DEVPROPKEY`, variable-
//! length interface-detail structs, HID preparsed data, registry parsing).

#![allow(non_snake_case)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use windows::core::{GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Devices::Bluetooth::{BluetoothGetDeviceInfo, BLUETOOTH_DEVICE_INFO};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, SetupDiGetDevicePropertyW,
    SetupDiGetDeviceRegistryPropertyW, SetupDiOpenDevRegKey, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    HDEVINFO, SETUP_DI_REGISTRY_PROPERTY, SPDRP_FRIENDLYNAME, SPDRP_HARDWAREID,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetHidGuid, HidD_GetManufacturerString,
    HidD_GetPreparsedData, HidD_GetProductString, HidP_GetCaps, HIDD_ATTRIBUTES, HIDP_CAPS,
    PHIDP_PREPARSED_DATA,
};
use windows::Win32::Devices::Properties::{DEVPROPKEY, DEVPROPTYPE};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_SHARING_VIOLATION, ERROR_SUCCESS, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    REG_VALUE_TYPE,
};

// Device interface class GUIDs (see winBindings.setupapi).
const GUID_CLASS_COMPORT: GUID = GUID::from_u128(0x86e0d1e0_8089_11d0_9ce4_08003e301f73);
const GUID_DEVINTERFACE_USB_DEVICE: GUID = GUID::from_u128(0xa5dcbf10_6530_11d2_901f_00c04fb951ed);
// DEVPKEY_Device_BusReportedDeviceDesc.
const DEVPKEY_BUS_REPORTED_DESC: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x540b947e_8b40_45bc_a8a2_6a0b894cbda2),
    pid: 4,
};
// SetupDiOpenDevRegKey scope/type (winreg constants not projected by windows-rs).
const DICS_FLAG_GLOBAL: u32 = 1;
const DIREG_DEV: u32 = 1;

/// A serial (COM) port entry. Mirrors the dict from `listComPorts`; `Option`
/// fields are omitted from the Python dict when `None`.
pub struct ComPort {
    pub port: String,
    pub friendly_name: String,
    pub hardware_id: Option<String>,
    pub bluetooth_address: Option<u64>,
    pub bluetooth_name: Option<String>,
    pub usb_id: Option<String>,
}

/// A USB device entry (`listUsbDevices`).
pub struct UsbDevice {
    pub hardware_id: Option<String>,
    pub usb_id: Option<String>,
    pub device_path: Option<String>,
    pub bus_reported_device_description: Option<String>,
}

/// A HID device entry (`listHidDevices`). `provider` is always present in the
/// Python dict (possibly `None`); the rest are conditional.
pub struct HidDevice {
    pub hardware_id: String,
    pub device_path: String,
    pub provider: Option<String>,
    pub usb_id: Option<String>,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub version_number: Option<u16>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub hid_usage_page: Option<u16>,
}

/// Extra HID info cached per device path, so a device that is currently open
/// (returns SHARING_VIOLATION on the next enumeration) still reports its
/// attributes. Matches `_getHidInfoCache` in the Python.
#[derive(Clone, Default)]
struct HidExtra {
    vendor_id: Option<u16>,
    product_id: Option<u16>,
    version_number: Option<u16>,
    manufacturer: Option<String>,
    product: Option<String>,
    hid_usage_page: Option<u16>,
}

static HID_CACHE: Mutex<BTreeMap<String, HidExtra>> = Mutex::new(BTreeMap::new());

// ---- wide-string helpers ----

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Decode a little-endian UTF-16 byte buffer up to the first NUL.
fn wide_bytes_to_string(buf: &[u8]) -> String {
    let mut units = Vec::with_capacity(buf.len() / 2);
    for ch in buf.chunks_exact(2) {
        let c = u16::from_le_bytes([ch[0], ch[1]]);
        if c == 0 {
            break;
        }
        units.push(c);
    }
    String::from_utf16_lossy(&units)
}

/// Decode a fixed `[u16]` array up to the first NUL.
fn wide_array_to_string(arr: &[u16]) -> String {
    let len = arr.iter().position(|&c| c == 0).unwrap_or(arr.len());
    String::from_utf16_lossy(&arr[..len])
}

// ---- SetupAPI enumeration core ----

/// Enumerate the device interfaces of `class_guid`, calling `f` with the
/// device-info set, the device's `SP_DEVINFO_DATA`, and its interface path.
/// Mirrors `_listDevices` (eager, since every caller collects to a list).
fn for_each_device_interface<F>(class_guid: &GUID, only_available: bool, mut f: F)
where
    F: FnMut(HDEVINFO, &SP_DEVINFO_DATA, String),
{
    let mut flags = DIGCF_DEVICEINTERFACE;
    if only_available {
        flags |= DIGCF_PRESENT;
    }
    let hdevinfo =
        match unsafe { SetupDiGetClassDevsW(Some(class_guid), PCWSTR::null(), None, flags) } {
            Ok(h) if !h.is_invalid() => h,
            _ => return,
        };
    for index in 0..256u32 {
        let mut did = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        // Stop at the end of the list (ERROR_NO_MORE_ITEMS) or on any error.
        if unsafe { SetupDiEnumDeviceInterfaces(hdevinfo, None, class_guid, index, &mut did) }
            .is_err()
        {
            break;
        }

        // First call: required detail-buffer size in bytes.
        let mut needed = 0u32;
        let _ = unsafe {
            SetupDiGetDeviceInterfaceDetailW(hdevinfo, &did, None, 0, Some(&mut needed), None)
        };
        if needed == 0 {
            continue;
        }
        let mut buf = vec![0u8; needed as usize];
        // cbSize is the documented struct size (8 on x64), not `needed`.
        let cb = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        buf[..4].copy_from_slice(&cb.to_le_bytes());
        let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;

        let mut devinfo = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        if unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                hdevinfo,
                &did,
                Some(detail),
                needed,
                None,
                Some(&mut devinfo),
            )
        }
        .is_err()
        {
            continue;
        }
        let off = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
        let path = wide_bytes_to_string(&buf[off..]);
        f(hdevinfo, &devinfo, path);
    }
    unsafe {
        let _ = SetupDiDestroyDeviceInfoList(hdevinfo);
    }
}

/// Read a string device registry property (SPDRP_*), first NUL-terminated
/// string only (HARDWAREID is REG_MULTI_SZ). Matches the Python 1024-WCHAR buffer.
fn get_registry_property(
    hdevinfo: HDEVINFO,
    devinfo: &SP_DEVINFO_DATA,
    prop: SETUP_DI_REGISTRY_PROPERTY,
) -> Option<String> {
    let mut buf = [0u8; 2048];
    if unsafe {
        SetupDiGetDeviceRegistryPropertyW(hdevinfo, devinfo, prop, None, Some(&mut buf), None)
    }
    .is_ok()
    {
        Some(wide_bytes_to_string(&buf))
    } else {
        None
    }
}

/// Read the bus-reported device description (a DEVPKEY string property).
fn get_bus_reported_description(
    hdevinfo: HDEVINFO,
    devinfo: &SP_DEVINFO_DATA,
) -> Option<String> {
    let mut buf = [0u8; 2048];
    let mut proptype = DEVPROPTYPE::default();
    if unsafe {
        SetupDiGetDevicePropertyW(
            hdevinfo,
            devinfo,
            &DEVPKEY_BUS_REPORTED_DESC,
            &mut proptype,
            Some(&mut buf),
            None,
            0,
        )
    }
    .is_ok()
    {
        Some(wide_bytes_to_string(&buf))
    } else {
        None
    }
}

// ---- registry helpers ----

fn reg_query_string(hkey: HKEY, name: &str) -> Option<String> {
    let wname = to_wide(name);
    let mut size = 0u32;
    let mut ty = REG_VALUE_TYPE::default();
    let _ = unsafe {
        RegQueryValueExW(hkey, PCWSTR(wname.as_ptr()), None, Some(&mut ty), None, Some(&mut size))
    };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let r = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(wname.as_ptr()),
            None,
            Some(&mut ty),
            Some(buf.as_mut_ptr()),
            Some(&mut size),
        )
    };
    if r == ERROR_SUCCESS {
        Some(wide_bytes_to_string(&buf[..size as usize]))
    } else {
        None
    }
}

fn reg_query_binary(hkey: HKEY, name: &str) -> Option<Vec<u8>> {
    let wname = to_wide(name);
    let mut size = 0u32;
    let mut ty = REG_VALUE_TYPE::default();
    let _ = unsafe {
        RegQueryValueExW(hkey, PCWSTR(wname.as_ptr()), None, Some(&mut ty), None, Some(&mut size))
    };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let r = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(wname.as_ptr()),
            None,
            Some(&mut ty),
            Some(buf.as_mut_ptr()),
            Some(&mut size),
        )
    };
    if r == ERROR_SUCCESS {
        buf.truncate(size as usize);
        Some(buf)
    } else {
        None
    }
}

/// RAII wrapper closing an HKEY on drop.
struct RegKey(HKEY);
impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn reg_open_hkcu(path: &str) -> Option<RegKey> {
    let wpath = to_wide(path);
    let mut hkey = HKEY::default();
    let r = unsafe {
        RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wpath.as_ptr()), 0, KEY_READ, &mut hkey)
    };
    if r == ERROR_SUCCESS {
        Some(RegKey(hkey))
    } else {
        None
    }
}

fn reg_open_subkey(parent: HKEY, name: &str) -> Option<RegKey> {
    let wname = to_wide(name);
    let mut hkey = HKEY::default();
    let r =
        unsafe { RegOpenKeyExW(parent, PCWSTR(wname.as_ptr()), 0, KEY_READ, &mut hkey) };
    if r == ERROR_SUCCESS {
        Some(RegKey(hkey))
    } else {
        None
    }
}

fn reg_enum_key_names(hkey: HKEY) -> Vec<String> {
    let mut out = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        let r = unsafe {
            RegEnumKeyExW(hkey, index, PWSTR(name.as_mut_ptr()), &mut len, None, PWSTR::null(), None, None)
        };
        if r != ERROR_SUCCESS {
            break;
        }
        out.push(wide_array_to_string(&name));
    }
    out
}

// ---- Bluetooth ----

/// Resolve a Bluetooth device's friendly name (`getBluetoothDeviceInfo().szName`).
fn bluetooth_device_name(address: u64) -> Option<String> {
    let mut info = BLUETOOTH_DEVICE_INFO {
        dwSize: size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
        ..Default::default()
    };
    let res = unsafe {
        info.Address.Anonymous.ullLong = address;
        BluetoothGetDeviceInfo(None, &mut info)
    };
    if res == ERROR_SUCCESS.0 {
        Some(wide_array_to_string(&info.szName))
    } else {
        None
    }
}

/// Parse a Microsoft BTHENUM `Bluetooth_UniqueID` into a numeric address:
/// after the first `#`, up to the first `_`, hex.
fn parse_bthenum_addr(unique_id: &str) -> Option<u64> {
    let after_hash = unique_id.split_once('#')?.1;
    let hex = after_hash.split_once('_').map(|(a, _)| a).unwrap_or(after_hash);
    u64::from_str_radix(hex, 16).ok()
}

/// Toshiba Bluetooth stack: map a COM port to (address, name) via HKCU. Legacy.
fn toshiba_bt_port_info(port: &str) -> Option<(u64, String)> {
    let root = reg_open_hkcu(r"Software\Toshiba\BluetoothStack\V1.0\EZC\DATA")?;
    for key_name in reg_enum_key_names(root.0) {
        let Some(item) = reg_open_subkey(root.0, &key_name) else {
            continue;
        };
        if let Some(scorig) = reg_open_subkey(item.0, "SCORIGINAL") {
            match reg_query_string(scorig.0, "PORTNAME") {
                Some(p) if p.trim_end_matches('\0') == port => {}
                _ => continue,
            }
        } else {
            continue;
        }
        let addr = reg_query_binary(item.0, "BDADDR").map(bytes_to_addr_reversed)?;
        let name = reg_query_string(item.0, "FRIENDLYNAME")?
            .trim_end_matches('\0')
            .to_string();
        return Some((addr, name));
    }
    None
}

/// Widcomm Bluetooth stack: map a COM port to (address, name) via HKCU. Legacy.
fn widcomm_bt_port_info(port: &str) -> Option<(u64, String)> {
    let root = reg_open_hkcu(r"Software\Widcomm\BTConfig\AutoConnect")?;
    let port_num = port.get(3..)?; // strip "COM"
    for key_name in reg_enum_key_names(root.0) {
        if key_name.trim_start_matches('0') != port_num {
            continue;
        }
        let Some(item) = reg_open_subkey(root.0, &key_name) else {
            continue;
        };
        let addr = reg_query_binary(item.0, "BDAddress").map(bytes_to_addr_reversed)?;
        let name = reg_query_string(item.0, "BDName")?;
        return Some((addr, name));
    }
    None
}

/// Convert a raw byte address (big-endian bytes) to a number, matching the
/// Python `sum(ord(byte) << (n*8) for n, byte in enumerate(reversed(addr)))`.
fn bytes_to_addr_reversed(bytes: Vec<u8>) -> u64 {
    bytes
        .iter()
        .rev()
        .enumerate()
        .fold(0u64, |acc, (n, &b)| acc + ((b as u64) << (n * 8)))
}

struct PortInfo {
    port: String,
    bluetooth_address: Option<u64>,
    bluetooth_name: Option<String>,
    usb_id: Option<String>,
}

/// Port + Bluetooth/USB info from a device's registry key (`_getBluetoothPortInfo`).
fn get_bluetooth_port_info(reg_key: HKEY, hw_id: &str) -> Option<PortInfo> {
    let port = reg_query_string(reg_key, "PortName")?;
    if port.is_empty() {
        return None;
    }
    let mut info = PortInfo {
        port: port.clone(),
        bluetooth_address: None,
        bluetooth_name: None,
        usb_id: None,
    };
    if hw_id.starts_with("BTHENUM\\") {
        if let Some(uid) = reg_query_string(reg_key, "Bluetooth_UniqueID") {
            if let Some(addr) = parse_bthenum_addr(&uid) {
                info.bluetooth_address = Some(addr);
                if addr != 0 {
                    info.bluetooth_name = bluetooth_device_name(addr);
                }
            }
        }
    } else if hw_id == "Bluetooth\\0004&0002" {
        if let Some((addr, name)) = toshiba_bt_port_info(&port) {
            info.bluetooth_address = Some(addr);
            info.bluetooth_name = Some(name);
        }
    } else if hw_id == "{95C7A0A0-3094-11D7-A202-00508B9D7D5A}\\BLUETOOTHPORT" {
        if let Some((addr, name)) = widcomm_bt_port_info(&port) {
            info.bluetooth_address = Some(addr);
            info.bluetooth_name = Some(name);
        }
    } else if hw_id.contains("USB") || hw_id.contains("FTDIBUS") {
        if let Some(i) = hw_id.find("VID_") {
            if let Some(usb) = hw_id.get(i..i + 17) {
                info.usb_id = Some(usb.to_string());
            }
        }
    }
    Some(info)
}

// ---- public enumeration functions ----

/// List COM ports (`listComPorts`).
pub fn list_com_ports(only_available: bool) -> Vec<ComPort> {
    let mut out = Vec::new();
    for_each_device_interface(&GUID_CLASS_COMPORT, only_available, |hdevinfo, devinfo, _path| {
        let hardware_id = get_registry_property(hdevinfo, devinfo, SPDRP_HARDWAREID);
        let reg_key = match unsafe {
            SetupDiOpenDevRegKey(hdevinfo, devinfo, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ.0)
        } {
            Ok(k) if !k.is_invalid() => RegKey(k),
            _ => return,
        };
        let Some(port_info) =
            get_bluetooth_port_info(reg_key.0, hardware_id.as_deref().unwrap_or(""))
        else {
            return;
        };
        drop(reg_key);
        let friendly_name = get_registry_property(hdevinfo, devinfo, SPDRP_FRIENDLYNAME)
            .unwrap_or_else(|| port_info.port.clone());
        out.push(ComPort {
            port: port_info.port,
            friendly_name,
            hardware_id,
            bluetooth_address: port_info.bluetooth_address,
            bluetooth_name: port_info.bluetooth_name,
            usb_id: port_info.usb_id,
        });
    });
    out
}

/// List USB devices (`listUsbDevices`).
pub fn list_usb_devices(only_available: bool) -> Vec<UsbDevice> {
    let mut out = Vec::new();
    for_each_device_interface(
        &GUID_DEVINTERFACE_USB_DEVICE,
        only_available,
        |hdevinfo, devinfo, path| {
            let hardware_id = get_registry_property(hdevinfo, devinfo, SPDRP_HARDWAREID);
            let (hw, usb_id, device_path) = match &hardware_id {
                // hardwareID is "usb\VID_xxxx&PID_xxxx&..."; usbID is chars 4..21.
                Some(h) => (
                    Some(h.clone()),
                    h.get(4..21).map(str::to_string),
                    Some(path),
                ),
                None => (None, None, None),
            };
            let bus_reported = get_bus_reported_description(hdevinfo, devinfo);
            out.push(UsbDevice {
                hardware_id: hw,
                usb_id,
                device_path,
                bus_reported_device_description: bus_reported,
            });
        },
    );
    out
}

/// List HID devices (`listHidDevices`).
pub fn list_hid_devices(only_available: bool) -> Vec<HidDevice> {
    let hid_guid = unsafe { HidD_GetHidGuid() };
    let mut out = Vec::new();
    for_each_device_interface(&hid_guid, only_available, |hdevinfo, devinfo, path| {
        if let Some(hw_id) = get_registry_property(hdevinfo, devinfo, SPDRP_HARDWAREID) {
            out.push(get_hid_info(hw_id, path));
        }
    });
    out
}

/// Build a HID device entry (`_getHidInfo`): provider classification plus, for
/// USB/Bluetooth HIDs, attributes/strings/usage read by opening the device
/// (with a per-path cache for devices that are currently open).
fn get_hid_info(hw_id: String, path: String) -> HidDevice {
    let mut dev = HidDevice {
        hardware_id: hw_id.clone(),
        device_path: path.clone(),
        provider: None,
        usb_id: None,
        vendor_id: None,
        product_id: None,
        version_number: None,
        manufacturer: None,
        product: None,
        hid_usage_page: None,
    };
    // hwId after the first backslash.
    let rest = hw_id.split_once('\\').map(|(_, b)| b).unwrap_or(&hw_id);
    if rest.starts_with("VID") {
        dev.provider = Some("usb".to_string());
        dev.usb_id = rest.get(..17).map(str::to_string);
    } else if rest.starts_with("{00001124-0000-1000-8000-00805f9b34fb}")
        || rest.starts_with("{00001812-0000-1000-8000-00805f9b34fb}")
    {
        // Classic or low-energy Bluetooth (#15470).
        dev.provider = Some("bluetooth".to_string());
    } else {
        // Unknown provider: no further info.
        return dev;
    }

    // Open the device to read extra info. Read-only, shared.
    let wpath = to_wide(&path);
    let handle = match unsafe {
        CreateFileW(
            PCWSTR(wpath.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        )
    } {
        Ok(h) if !h.is_invalid() => h,
        Ok(_) => return dev,
        Err(e) => {
            // If the device is in use, fall back to the cached info.
            if e.code() == HRESULT::from_win32(ERROR_SHARING_VIOLATION.0) {
                if let Some(extra) = HID_CACHE.lock().unwrap().get(&path) {
                    apply_hid_extra(&mut dev, extra);
                }
            }
            return dev;
        }
    };

    let extra = read_hid_extra(handle);
    unsafe {
        let _ = CloseHandle(handle);
    }
    apply_hid_extra(&mut dev, &extra);
    HID_CACHE.lock().unwrap().insert(path, extra);
    dev
}

fn apply_hid_extra(dev: &mut HidDevice, extra: &HidExtra) {
    dev.vendor_id = extra.vendor_id;
    dev.product_id = extra.product_id;
    dev.version_number = extra.version_number;
    dev.manufacturer = extra.manufacturer.clone();
    dev.product = extra.product.clone();
    dev.hid_usage_page = extra.hid_usage_page;
}

/// Read HID attributes, manufacturer/product strings and the top-level usage
/// page from an open HID device handle.
fn read_hid_extra(handle: HANDLE) -> HidExtra {
    let mut extra = HidExtra::default();
    let mut attribs = HIDD_ATTRIBUTES {
        Size: size_of::<HIDD_ATTRIBUTES>() as u32,
        ..Default::default()
    };
    if unsafe { HidD_GetAttributes(handle, &mut attribs) }.as_bool() {
        extra.vendor_id = Some(attribs.VendorID);
        extra.product_id = Some(attribs.ProductID);
        extra.version_number = Some(attribs.VersionNumber);
    }
    let mut buf = [0u16; 128];
    let bytes = (buf.len() * 2) as u32;
    if unsafe { HidD_GetManufacturerString(handle, buf.as_mut_ptr() as *mut _, bytes) }.as_bool() {
        extra.manufacturer = Some(wide_array_to_string(&buf));
    }
    if unsafe { HidD_GetProductString(handle, buf.as_mut_ptr() as *mut _, bytes) }.as_bool() {
        extra.product = Some(wide_array_to_string(&buf));
    }
    let mut preparsed = PHIDP_PREPARSED_DATA::default();
    if unsafe { HidD_GetPreparsedData(handle, &mut preparsed) }.as_bool() {
        let mut caps = HIDP_CAPS::default();
        if unsafe { HidP_GetCaps(preparsed, &mut caps) }.is_ok() {
            extra.hid_usage_page = Some(caps.UsagePage);
        }
        unsafe {
            let _ = HidD_FreePreparsedData(preparsed);
        }
    }
    extra
}
