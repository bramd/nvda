# A part of NonVisual Desktop Access (NVDA)
# Copyright (C) 2001-2025 Chris Liechti, NV Access Limited, Babbage B.V., Leonard de Ruijter
# Based on serial scanner code by Chris Liechti from https://raw.githubusercontent.com/pyserial/pyserial/81167536e796cc2e13aa16abd17a14634dc3aed1/pyserial/examples/scanwin32.py

"""Utilities for working with hardware connection ports.

The device enumeration (SetupAPI / HID / Bluetooth) is implemented in Rust and
exposed as ``nvdaRust.hwportutils``; the functions here delegate to it.
"""

import ctypes
import typing

import nvdaRust
import utils._deprecate


def _ValidHandle(value):
	if value == 0:
		raise ctypes.WinError()
	return value


def listComPorts(onlyAvailable: bool = True) -> typing.Iterator[dict]:
	"""List com ports on the system.
	:param onlyAvailable: Only return ports that are currently available.
	:return: Dicts including keys of port, friendlyName and hardwareID.
	"""
	return iter(nvdaRust.hwportutils.listComPorts(onlyAvailable))


def listUsbDevices(onlyAvailable: bool = True) -> typing.Iterator[dict]:
	"""List USB devices on the system.
	:param onlyAvailable: Only return devices that are currently available.
	:return: Generates dicts including keys of usbID (VID and PID), devicePath and hardwareID.
	"""
	return iter(nvdaRust.hwportutils.listUsbDevices(onlyAvailable))


def listHidDevices(onlyAvailable: bool = True) -> typing.Iterator[dict]:
	"""List HID devices on the system.
	@param onlyAvailable: Only return devices that are currently available.
	@return: Generates dicts including keys such as hardwareID,
		usbID (in the form "VID_xxxx&PID_xxxx")
		and devicePath.
	"""
	return iter(nvdaRust.hwportutils.listHidDevices(onlyAvailable))


__getattr__ = utils._deprecate.handleDeprecations(
	# Now in winBindings.advapi32
	utils._deprecate.MovedSymbol("RegCloseKey", "winBindings.advapi32"),
	# Now in winBindings.bthprops
	utils._deprecate.MovedSymbol("BLUETOOTH_ADDRESS", "winBindings.bthprops"),
	utils._deprecate.MovedSymbol("BLUETOOTH_DEVICE_INFO", "winBindings.bthprops"),
	utils._deprecate.MovedSymbol("BLUETOOTH_MAX_NAME_SIZE", "winBindings.bthprops"),
	utils._deprecate.MovedSymbol("BluetoothGetDeviceInfo", "winBindings.bthprops"),
	utils._deprecate.MovedSymbol("BTH_ADDR", "winBindings.bthprops", "BLUETOOTH_ADDRESS"),
	# Now in winBindings.cfgmgr32
	utils._deprecate.MovedSymbol("CM_Get_Device_ID", "winBindings.cfgmgr32"),
	utils._deprecate.MovedSymbol("CR_SUCCESS", "winBindings.cfgmgr32"),
	utils._deprecate.MovedSymbol("MAX_DEVICE_ID_LEN", "winBindings.cfgmgr32"),
	# Now in winBindings.hid
	utils._deprecate.MovedSymbol("HIDD_ATTRIBUTES", "winBindings.hid"),
	utils._deprecate.MovedSymbol("HidD_FreePreparsedData", "winBindings.hid"),
	utils._deprecate.MovedSymbol("HidD_GetAttributes", "winBindings.hid"),
	utils._deprecate.MovedSymbol("HidD_GetHidGuid", "winBindings.hid"),
	utils._deprecate.MovedSymbol("HidD_GetManufacturerString", "winBindings.hid"),
	utils._deprecate.MovedSymbol("HidD_GetPreparsedData", "winBindings.hid"),
	utils._deprecate.MovedSymbol("HidD_GetProductString", "winBindings.hid"),
	utils._deprecate.MovedSymbol("HidP_GetCaps", "winBindings.hid"),
	# Now in winBindings.setupapi
	utils._deprecate.MovedSymbol("DEVPKEY_Device_BusReportedDeviceDesc", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("DEVPROPKEY", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("DICS_FLAG_GLOBAL", "winBindings.setupapi", "DICS_FLAG", "GLOBAL"),
	utils._deprecate.MovedSymbol("DIGCF_DEVICEINTERFACE", "winBindings.setupapi", "DIGCF", "DEVICEINTERFACE"),
	utils._deprecate.MovedSymbol("DIGCF_PRESENT", "winBindings.setupapi", "DIGCF", "PRESENT"),
	utils._deprecate.MovedSymbol("DIREG_DEV", "winBindings.setupapi", "DIREG", "DEV"),
	utils._deprecate.MovedSymbol("dummy", "winBindings.setupapi", "_Dummy"),
	utils._deprecate.MovedSymbol("GUID_CLASS_COMPORT", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("GUID_DEVINTERFACE_USB_DEVICE", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("HDEVINFO", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("PSP_DEVICE_INTERFACE_DATA", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("PSP_DEVICE_INTERFACE_DETAIL_DATA", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("PSP_DEVINFO_DATA", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SetupDiDestroyDeviceInfoList", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SetupDiEnumDeviceInfo", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SetupDiEnumDeviceInterfaces", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SetupDiGetClassDevs", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SetupDiGetDeviceInterfaceDetail", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SetupDiGetDeviceProperty", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SetupDiGetDeviceRegistryProperty", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SetupDiOpenDevRegKey", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SIZEOF_SP_DEVICE_INTERFACE_DETAIL_DATA_W", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SP_DEVICE_INTERFACE_DATA", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SP_DEVINFO_DATA", "winBindings.setupapi"),
	utils._deprecate.MovedSymbol("SPDRP_DEVICEDESC", "winBindings.setupapi", "SPDRP", "DEVICEDESC"),
	utils._deprecate.MovedSymbol("SPDRP_FRIENDLYNAME", "winBindings.setupapi", "SPDRP", "FRIENDLYNAME"),
	utils._deprecate.MovedSymbol("SPDRP_HARDWAREID", "winBindings.setupapi", "SPDRP", "HARDWAREID"),
	utils._deprecate.MovedSymbol(
		"SPDRP_LOCATION_INFORMATION",
		"winBindings.setupapi",
		"SPDRP",
		"LOCATION_INFORMATION",
	),
	# Now in winAPI.constants
	utils._deprecate.MovedSymbol(
		"ERROR_INSUFFICIENT_BUFFER",
		"winAPI.constants",
		"SystemErrorCodes",
		"INSUFFICIENT_BUFFER",
	),
	utils._deprecate.MovedSymbol(
		"ERROR_NO_MORE_ITEMS",
		"winAPI.constants",
		"SystemErrorCodes",
		"NO_MORE_ITEMS",
	),
	# No longer part of the public API
	utils._deprecate.RemovedSymbol("INVALID_HANDLE_VALUE", 0),
	utils._deprecate.RemovedSymbol("ValidHandle", _ValidHandle),
)
"""Module __getattr__ to handle backward compatibility."""
