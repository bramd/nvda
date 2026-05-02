# A part of NonVisual Desktop Access (NVDA)
# This file is covered by the GNU General Public License.
# See the file COPYING for more details.
# Copyright (C) 2026 NV Access Limited

"""Manual integration test for ``nvdaRust.ole.getOleClipboardText``.

The function takes a real COM ``IUnknown*`` so it can't be unit-tested in
isolation without a custom COM server. The cleanest synthetic driver is the
system clipboard: ``OleGetClipboard`` returns a real ``IDataObject`` backed by
whatever the OS holds, exercising the same code path Outlook hits when reading
embedded-object data.

Verifies:
  * Round-trips ASCII via ``IDataObject::GetData(CF_UNICODETEXT)``
  * Round-trips CJK and ZWJ-emoji (the Rust UTF-16 → ``String`` path)
  * Returns ``""`` for empty text
  * Raises ``OSError`` when given a NULL ``IUnknown*``

Usage::

    uv run python tests/manual/rust/oleIntegration.py

The script overwrites the system clipboard during execution and best-effort
restores any prior CF_UNICODETEXT content on exit. Other clipboard formats
(images, files) are not preserved.

Exit code: 0 on all pass, 1 on any failure.
"""

import ctypes
import sys

import nvdaRust


CF_UNICODETEXT = 13
GMEM_MOVEABLE = 0x0002

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
	"""Get an IDataObject* for the current clipboard; return its address as int."""
	pUnk = ctypes.c_void_p()
	hr = ole32.OleGetClipboard(ctypes.byref(pUnk))
	if hr != 0:
		raise OSError(f"OleGetClipboard failed: HRESULT 0x{hr & 0xFFFFFFFF:08x}")
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


def runNullIUnknownCase() -> bool:
	try:
		nvdaRust.ole.getOleClipboardText(0)
	except OSError as e:
		print(f"  PASS  null IUnknown raises OSError ({e})")
		return True
	print("  FAIL  null IUnknown should have raised OSError")
	return False


def main() -> int:
	ole32.OleInitialize(None)
	saved = getClipboardUnicode()
	failures = 0
	try:
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
		if not runNullIUnknownCase():
			failures += 1
	finally:
		if saved is not None:
			try:
				setClipboardUnicode(saved)
			except OSError:
				pass
		ole32.OleUninitialize()
	print()
	print(f"{'PASS' if failures == 0 else 'FAIL'} ({failures} failure(s))")
	return 0 if failures == 0 else 1


if __name__ == "__main__":
	sys.exit(main())
