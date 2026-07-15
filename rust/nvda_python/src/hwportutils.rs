#![allow(non_snake_case)]

use pyo3::prelude::*;
use pyo3::types::PyDict;

/// List COM ports as dicts (keys: port, friendlyName, hardwareID, and
/// optionally bluetoothAddress, bluetoothName, usbID). Mirrors
/// `hwPortUtils.listComPorts`.
#[pyfunction]
#[pyo3(name = "listComPorts", signature = (onlyAvailable = true))]
pub fn list_com_ports(py: Python<'_>, onlyAvailable: bool) -> PyResult<Vec<Py<PyDict>>> {
    // Enumeration does blocking Win32/registry calls; release the GIL for it.
    let ports = py.detach(|| nvda_hwportutils::list_com_ports(onlyAvailable));
    ports
        .into_iter()
        .map(|p| {
            let d = PyDict::new(py);
            d.set_item("port", p.port)?;
            d.set_item("friendlyName", p.friendly_name)?;
            if let Some(v) = p.hardware_id {
                d.set_item("hardwareID", v)?;
            }
            if let Some(v) = p.bluetooth_address {
                d.set_item("bluetoothAddress", v)?;
            }
            if let Some(v) = p.bluetooth_name {
                d.set_item("bluetoothName", v)?;
            }
            if let Some(v) = p.usb_id {
                d.set_item("usbID", v)?;
            }
            Ok(d.unbind())
        })
        .collect()
}

/// List USB devices as dicts (keys: hardwareID, usbID, devicePath, and
/// optionally busReportedDeviceDescription). Mirrors `listUsbDevices`.
#[pyfunction]
#[pyo3(name = "listUsbDevices", signature = (onlyAvailable = true))]
pub fn list_usb_devices(py: Python<'_>, onlyAvailable: bool) -> PyResult<Vec<Py<PyDict>>> {
    let devices = py.detach(|| nvda_hwportutils::list_usb_devices(onlyAvailable));
    devices
        .into_iter()
        .map(|dev| {
            let d = PyDict::new(py);
            if let Some(v) = dev.hardware_id {
                d.set_item("hardwareID", v)?;
            }
            if let Some(v) = dev.usb_id {
                d.set_item("usbID", v)?;
            }
            if let Some(v) = dev.device_path {
                d.set_item("devicePath", v)?;
            }
            if let Some(v) = dev.bus_reported_device_description {
                d.set_item("busReportedDeviceDescription", v)?;
            }
            Ok(d.unbind())
        })
        .collect()
}

/// List HID devices as dicts (keys: hardwareID, devicePath, provider [always,
/// possibly None], and optionally usbID, vendorID, productID, versionNumber,
/// manufacturer, product, HIDUsagePage). Mirrors `listHidDevices`.
#[pyfunction]
#[pyo3(name = "listHidDevices", signature = (onlyAvailable = true))]
pub fn list_hid_devices(py: Python<'_>, onlyAvailable: bool) -> PyResult<Vec<Py<PyDict>>> {
    let devices = py.detach(|| nvda_hwportutils::list_hid_devices(onlyAvailable));
    devices
        .into_iter()
        .map(|dev| {
            let d = PyDict::new(py);
            d.set_item("hardwareID", dev.hardware_id)?;
            d.set_item("devicePath", dev.device_path)?;
            // provider is always present (may be None).
            d.set_item("provider", dev.provider)?;
            if let Some(v) = dev.usb_id {
                d.set_item("usbID", v)?;
            }
            if let Some(v) = dev.vendor_id {
                d.set_item("vendorID", v)?;
            }
            if let Some(v) = dev.product_id {
                d.set_item("productID", v)?;
            }
            if let Some(v) = dev.version_number {
                d.set_item("versionNumber", v)?;
            }
            if let Some(v) = dev.manufacturer {
                d.set_item("manufacturer", v)?;
            }
            if let Some(v) = dev.product {
                d.set_item("product", v)?;
            }
            if let Some(v) = dev.hid_usage_page {
                d.set_item("HIDUsagePage", v)?;
            }
            Ok(d.unbind())
        })
        .collect()
}
