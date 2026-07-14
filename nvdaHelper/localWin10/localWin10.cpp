/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2026 NV Access Limited
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

/*
 * nvdaHelperLocalWin10's engines are implemented in Rust (nvda_uwp_ocr for
 * UWP OCR, nvda_onecore_speech for OneCore speech). This translation unit is
 * the DLL's only C++ input; it exists to anchor the MSVC C runtime (/MT) and
 * the target machine for the link, and to provide DllMain, since the Rust
 * staticlibs alone do not pull in the CRT or a DLL entry point.
 */

#include <windows.h>

BOOL WINAPI DllMain(HINSTANCE hinstDLL, DWORD fdwReason, LPVOID lpvReserved) {
	return TRUE;
}
