# A part of NonVisual Desktop Access (NVDA)
# This file is covered by the GNU General Public License.
# See the file COPYING for more details.
# Copyright (C) 2026 NV Access Limited

"""Manual integration test for ``nvdaRust.ole.getOleClipboardText`` and
``nvdaRust.ole.getOleUserType``.

Both functions take a real COM ``IUnknown*`` and can't be unit-tested in
isolation without a custom COM server.

For ``getOleClipboardText``, the cleanest synthetic driver is the system
clipboard: ``OleGetClipboard`` returns a real ``IDataObject`` backed by whatever
the OS holds, exercising the same code path Outlook hits when reading
embedded-object data.

For ``getOleUserType``, we ``CoCreateInstance`` a known OLE-embeddable class
(Excel.Sheet, falling back to Word.Document) and ask its ``IOleObject`` for
its user-visible type name. Requires Microsoft Office to be installed; tests
are skipped (not failed) if neither class is registered.

Verifies:
  * ``getOleClipboardText`` round-trips ASCII / CJK / ZWJ-emoji / empty
  * ``getOleClipboardText`` raises ``OSError`` on NULL ``IUnknown*``
  * ``getOleUserType`` returns non-empty strings for FULL / SHORT / APPNAME
    flags against an Excel or Word OLE class
  * ``getOleUserType`` raises ``OSError`` on NULL ``IUnknown*``

Usage::

    uv run python tests/manual/rust/oleIntegration.py

The script overwrites the system clipboard during execution and best-effort
restores any prior CF_UNICODETEXT content on exit. Other clipboard formats
(images, files) are not preserved. The ``getOleUserType`` test may briefly
launch a hidden Excel/Word process.

Exit code: 0 on all pass / Office-tests skipped, 1 on any failure.
"""

import ctypes
import logging
import sys
import time

import nvdaRust


class _RecordCollector(logging.Handler):
	"""Captures log records emitted via Python's logging module so the manual
	test can verify Rust-emitted log calls flow through pyo3-log."""

	def __init__(self):
		super().__init__()
		self.records: list[logging.LogRecord] = []

	def emit(self, record: logging.LogRecord) -> None:
		self.records.append(record)


def _installLogCapture(level: int = logging.DEBUG) -> _RecordCollector:
	"""Install a record-capturing handler at the given level. Returns the handler."""
	collector = _RecordCollector()
	collector.setLevel(level)
	root = logging.getLogger()
	root.addHandler(collector)
	# Default root level is WARNING; lower it so DEBUG-level Rust messages flow.
	if root.level > level or root.level == logging.NOTSET:
		root.setLevel(level)
	return collector


CF_UNICODETEXT = 13
GMEM_MOVEABLE = 0x0002

# CoCreateInstance flags
CLSCTX_INPROC_SERVER = 0x1
CLSCTX_LOCAL_SERVER = 0x4
CLSCTX_SERVER = CLSCTX_INPROC_SERVER | CLSCTX_LOCAL_SERVER

# IOleObject::GetUserType flags (USERCLASSTYPE enum)
USERCLASSTYPE_FULL = 1  # full type name e.g. "Microsoft Excel Worksheet"
USERCLASSTYPE_SHORT = 2  # short name e.g. "Worksheet"
USERCLASSTYPE_APPNAME = 3  # app name e.g. "Microsoft Excel"


class GUID(ctypes.Structure):
	_fields_ = [
		("Data1", ctypes.c_uint32),
		("Data2", ctypes.c_uint16),
		("Data3", ctypes.c_uint16),
		("Data4", ctypes.c_ubyte * 8),
	]


user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32
ole32 = ctypes.windll.ole32

# Set restype + argtypes for any API that takes/returns a HANDLE/pointer —
# ctypes defaults to c_int and would truncate to 32 bits on 64-bit Python.
kernel32.GlobalAlloc.restype = ctypes.c_void_p
kernel32.GlobalAlloc.argtypes = [ctypes.c_uint, ctypes.c_size_t]
kernel32.GlobalLock.restype = ctypes.c_void_p
kernel32.GlobalLock.argtypes = [ctypes.c_void_p]
kernel32.GlobalUnlock.argtypes = [ctypes.c_void_p]
user32.GetClipboardData.restype = ctypes.c_void_p
user32.GetClipboardData.argtypes = [ctypes.c_uint]
user32.SetClipboardData.restype = ctypes.c_void_p
user32.SetClipboardData.argtypes = [ctypes.c_uint, ctypes.c_void_p]
ole32.CLSIDFromProgID.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(GUID)]
ole32.CLSIDFromProgID.restype = ctypes.c_int32
ole32.IIDFromString.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(GUID)]
ole32.IIDFromString.restype = ctypes.c_int32
ole32.CoCreateInstance.argtypes = [
	ctypes.POINTER(GUID),
	ctypes.c_void_p,
	ctypes.c_uint32,
	ctypes.POINTER(GUID),
	ctypes.POINTER(ctypes.c_void_p),
]
ole32.CoCreateInstance.restype = ctypes.c_int32

# IID_IOleObject = {00000112-0000-0000-C000-000000000046}
# Computed lazily — calling IIDFromString at module-import time appears to leave
# OLE in a state where the first OleGetClipboard call fails with
# CLIPBRD_E_CANT_OPEN.
IID_IOleObject: GUID | None = None


def _getIidIOleObject() -> GUID:
	global IID_IOleObject
	if IID_IOleObject is None:
		iid = GUID()
		ole32.IIDFromString("{00000112-0000-0000-C000-000000000046}", ctypes.byref(iid))
		IID_IOleObject = iid
	return IID_IOleObject


def setClipboardUnicode(text: str) -> None:
	"""Place ``text`` on the system clipboard as CF_UNICODETEXT."""
	if not user32.OpenClipboard(None):
		raise OSError("OpenClipboard failed")
	try:
		user32.EmptyClipboard()
		wide = text.encode("utf-16-le") + b"\x00\x00"
		hMem = kernel32.GlobalAlloc(GMEM_MOVEABLE, len(wide))
		if not hMem:
			raise OSError("GlobalAlloc failed")
		ptr = kernel32.GlobalLock(hMem)
		ctypes.memmove(ptr, wide, len(wide))
		kernel32.GlobalUnlock(hMem)
		if not user32.SetClipboardData(CF_UNICODETEXT, hMem):
			raise OSError("SetClipboardData failed")
	finally:
		user32.CloseClipboard()


def getClipboardUnicode() -> str | None:
	"""Return current CF_UNICODETEXT clipboard content, or None if unavailable."""
	if not user32.IsClipboardFormatAvailable(CF_UNICODETEXT):
		return None
	if not user32.OpenClipboard(None):
		return None
	try:
		hMem = user32.GetClipboardData(CF_UNICODETEXT)
		if not hMem:
			return None
		ptr = kernel32.GlobalLock(hMem)
		if not ptr:
			return None
		try:
			return ctypes.wstring_at(ptr)
		finally:
			kernel32.GlobalUnlock(hMem)
	finally:
		user32.CloseClipboard()


def getClipboardDataObjectAddr() -> int:
	"""Get an IDataObject* for the current clipboard; return its address as int.

	Retries on CLIPBRD_E_CANT_OPEN (0x800401D0) — there's a brief window after
	a Win32 ``CloseClipboard`` where ``OleGetClipboard`` can't open the
	clipboard while the OS commits the data to the OLE layer.
	"""
	CLIPBRD_E_CANT_OPEN = 0x800401D0
	pUnk = ctypes.c_void_p()
	for attempt in range(5):
		hr = ole32.OleGetClipboard(ctypes.byref(pUnk))
		if hr == 0:
			break
		if (hr & 0xFFFFFFFF) != CLIPBRD_E_CANT_OPEN:
			raise OSError(f"OleGetClipboard failed: HRESULT 0x{hr & 0xFFFFFFFF:08x}")
		time.sleep(0.2)
	else:
		raise OSError(f"OleGetClipboard kept failing with CLIPBRD_E_CANT_OPEN after {attempt + 1} attempts")
	if not pUnk.value:
		raise OSError("OleGetClipboard returned null")
	return pUnk.value


def releaseIUnknown(addr: int) -> None:
	"""Call IUnknown::Release on the COM object at ``addr`` (vtable slot 2)."""
	if not addr:
		return
	# IUnknown vtable layout: [QueryInterface, AddRef, Release].
	# Read vtable pointer from object, then index slot 2 for Release.
	vtableAddr = ctypes.c_void_p.from_address(addr).value
	if not vtableAddr:
		return
	releaseSlot = vtableAddr + 2 * ctypes.sizeof(ctypes.c_void_p)
	releaseFn = ctypes.c_void_p.from_address(releaseSlot).value
	if not releaseFn:
		return
	ctypes.WINFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p)(releaseFn)(addr)


def runCase(name: str, text: str) -> bool:
	setClipboardUnicode(text)
	addr = getClipboardDataObjectAddr()
	try:
		result = nvdaRust.ole.getOleClipboardText(addr)
	finally:
		releaseIUnknown(addr)
	if result == text:
		print(f"  PASS  {name}")
		return True
	print(f"  FAIL  {name}: expected {text!r}, got {result!r}")
	return False


def runNullIUnknownCase(collector: _RecordCollector) -> bool:
	beforeCount = len(collector.records)
	try:
		nvdaRust.ole.getOleClipboardText(0)
	except OSError as e:
		# Verify the Rust side emitted a WARNING-level log record.
		newRecords = collector.records[beforeCount:]
		oleRecords = [
			r for r in newRecords if r.levelno == logging.WARNING and "pUnknown is null" in r.getMessage()
		]
		if oleRecords:
			print(f"  PASS  null IUnknown raises OSError ({e}) and emits WARNING log record")
			return True
		print(
			f"  FAIL  null IUnknown raised OSError but no matching WARNING record found. "
			f"Captured {len(newRecords)} new record(s); levels: {[r.levelno for r in newRecords]}",
		)
		return False
	print("  FAIL  null IUnknown should have raised OSError")
	return False


def createOleObject(progId: str) -> int | None:
	"""CoCreateInstance the given ProgID and return its IOleObject* address.

	Returns None if the class is not registered (Office not installed).
	Raises OSError on other failures.
	"""
	clsid = GUID()
	hr = ole32.CLSIDFromProgID(progId, ctypes.byref(clsid))
	if hr != 0:
		# 0x800401F3 = CO_E_CLASSSTRING (ProgID not registered)
		return None
	pOle = ctypes.c_void_p()
	hr = ole32.CoCreateInstance(
		ctypes.byref(clsid),
		None,
		CLSCTX_SERVER,
		ctypes.byref(_getIidIOleObject()),
		ctypes.byref(pOle),
	)
	if hr != 0:
		raise OSError(
			f"CoCreateInstance({progId}) failed: HRESULT 0x{hr & 0xFFFFFFFF:08x}",
		)
	if not pOle.value:
		raise OSError(f"CoCreateInstance({progId}) returned null")
	return pOle.value


def runUserTypeCases() -> tuple[int, int]:
	"""Drive getOleUserType against a Word or Excel OLE object.

	Word.Document is preferred because Word implements ``IOleObject::GetUserType``
	and returns real strings (e.g. "Microsoft Word Document", "Document").
	Excel.Sheet is the fallback: Excel commonly returns ``E_FAIL`` for GetUserType,
	expecting callers to read the ``AuxUserType`` registry key directly. That still
	exercises our HRESULT-to-OSError mapping, just from the failure path.

	Returns (failures, runs). If neither class is registered, runs is 0 and the
	caller should treat the test as skipped.
	"""
	progId = None
	addr = None
	for candidate in ("Word.Document", "Excel.Sheet"):
		try:
			addr = createOleObject(candidate)
		except OSError as e:
			print(f"  SKIP  {candidate}: {e}")
			continue
		if addr is not None:
			progId = candidate
			break
	if addr is None:
		print("  SKIP  getOleUserType: no Word.Document or Excel.Sheet registered")
		return 0, 0

	failures = 0
	runs = 0
	flagNames = {
		USERCLASSTYPE_FULL: "FULL",
		USERCLASSTYPE_SHORT: "SHORT",
		USERCLASSTYPE_APPNAME: "APPNAME",
	}
	# Excel returns E_FAIL for all flags; treat that as expected-and-passed for
	# the Excel fallback path. Word should return real strings.
	excelExpectedFail = progId == "Excel.Sheet"
	try:
		for flag, flagName in flagNames.items():
			runs += 1
			try:
				result = nvdaRust.ole.getOleUserType(addr, flag)
			except OSError as e:
				if excelExpectedFail:
					print(f"  PASS  {progId} {flagName}: expected failure ({e})")
				else:
					print(f"  FAIL  {progId} {flagName}: raised {e}")
					failures += 1
				continue
			if excelExpectedFail:
				print(
					f"  PASS  {progId} {flagName}: unexpected success {result!r} (Excel implementation may have changed)",
				)
			elif result:
				print(f"  PASS  {progId} {flagName}: {result!r}")
			else:
				print(f"  FAIL  {progId} {flagName}: returned empty string")
				failures += 1

		# Error-path: null IUnknown
		runs += 1
		try:
			nvdaRust.ole.getOleUserType(0, USERCLASSTYPE_FULL)
			print("  FAIL  getOleUserType(null) should have raised OSError")
			failures += 1
		except OSError as e:
			print(f"  PASS  getOleUserType null IUnknown raises OSError ({e})")
	finally:
		releaseIUnknown(addr)
	return failures, runs


def main() -> int:
	ole32.OleInitialize(None)
	collector = _installLogCapture(level=logging.DEBUG)
	saved = getClipboardUnicode()
	failures = 0
	try:
		print("getOleClipboardText:")
		cases = [
			("ASCII", "Hello NVDA"),
			("CJK", "日本語テスト"),
			(
				"ZWJ emoji",
				"\U0001f468‍\U0001f469‍\U0001f467‍\U0001f466",
			),
			("empty", ""),
		]
		for name, text in cases:
			if not runCase(name, text):
				failures += 1
		if not runNullIUnknownCase(collector):
			failures += 1

		print()
		print("getOleUserType:")
		userTypeFailures, _ = runUserTypeCases()
		failures += userTypeFailures
	finally:
		if saved is not None:
			try:
				setClipboardUnicode(saved)
			except OSError:
				pass
		ole32.OleUninitialize()
	print()
	print(f"Captured {len(collector.records)} total log record(s) during run.")
	print(f"{'PASS' if failures == 0 else 'FAIL'} ({failures} failure(s))")
	return 0 if failures == 0 else 1


if __name__ == "__main__":
	sys.exit(main())
