# A part of NonVisual Desktop Access (NVDA)
# This file is covered by the GNU General Public License.
# See the file COPYING for more details.
# Copyright (C) 2024-2025 NV Access Limited, Leonard de Ruijter

"""Functions for splitting text at character (grapheme cluster) boundaries."""

from typing import Generator


def splitAtCharacterBoundaries(text: str) -> Generator[str, None, None]:
	"""
	Splits a given string into real visible characters (or glyphs), thereby respecting character boundaries.
	Contrary to just iterating over a string, this respects surrogate pairs, decomposite characters, etc.
	"""
	if not text:
		return
	import nvdaRust

	yield from nvdaRust.text.splitAtCharacterBoundaries(text)
