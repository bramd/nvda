/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2010-2012 World Light Information Limited and Hong Kong Blind Union.
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#ifndef TSF_H
#define TSF_H

void TSF_inProcess_initialize();
void TSF_inProcess_terminate();
void TSF_thread_detached();
// C linkage so the Rust nvda_input_hooks staticlib can call this via an
// `extern "C"` declaration. The C++ definition in tsf.cpp picks up C linkage
// from this header.
#ifdef __cplusplus
extern "C"
#endif
bool isTSFThread();
extern CLSID curTSFClsID;

#endif
