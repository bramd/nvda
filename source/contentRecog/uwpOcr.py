# A part of NonVisual Desktop Access (NVDA)
# Copyright (C) 2017-2025 NV Access Limited, Cary-rowen
# This file is covered by the GNU General Public License.
# See the file COPYING for more details.

"""Recognition of text using the UWP OCR engine included in Windows 10 and later."""

import json
import nvdaRust
from . import ContentRecognizer, LinesWordsResult
import config
import languageHandler


def getLanguages():
	"""Return the available recognition languages.
	@return: A list of language codes suitable to be passed to L{UwpOcr}'s constructor.
		These need to be normalized with L{languageHandler.normalizeLanguage}
		for use as NVDA language codes.
	@rtype: list of str
	"""
	return nvdaRust.uwp_ocr.getLanguages()


def getInitialLanguage():
	"""Get the language to use the first time UWP OCR is used.
	The NVDA interface language is used if a matching OCR language is available.
	Otherwise, this falls back to the first available language.
	"""
	nvdaLang = languageHandler.getLanguage()
	ocrLangs = getLanguages()
	return _getInitialLanguage(nvdaLang, ocrLangs)


def _getInitialLanguage(nvdaLang, ocrLangs):
	# Try the full language code.
	for lang in ocrLangs:
		normLang = languageHandler.normalizeLanguage(lang)
		if nvdaLang == normLang:
			return lang
	# Try the language code without country.
	nvdaLangPrimary = nvdaLang.split("_", 1)[0]
	for lang in ocrLangs:
		# Don't need to normalize here because the primary code is
		# the same when normalized.
		if lang.startswith(nvdaLangPrimary):
			return lang
	# Fall back to the first OCR language.
	if len(ocrLangs) >= 1:
		return ocrLangs[0]
	raise LookupError("No UWP OCR languages installed")


def getConfigLanguage():
	"""Get the user's configured OCR language.
	If no language has been configured, choose an initial language
	and update the configuration.
	"""
	lang = config.conf["uwpOcr"]["language"]
	if lang:
		return lang
	initial = getInitialLanguage()
	config.conf["uwpOcr"]["language"] = initial
	return initial


class UwpOcr(ContentRecognizer):
	@classmethod
	def _get_allowAutoRefresh(cls) -> bool:
		return config.conf["uwpOcr"]["autoRefresh"]

	@classmethod
	def _get_autoRefreshInterval(cls) -> int:
		return config.conf["uwpOcr"]["autoRefreshInterval"]

	@classmethod
	def _get_autoSayAllOnResult(cls) -> bool:
		return config.conf["uwpOcr"]["autoSayAllOnResult"]

	def getResizeFactor(self, width, height):
		# UWP OCR performs poorly with small images, so increase their size.
		if width < 100 or height < 100:
			return 4
		return 1

	def __init__(self, language=None):
		"""
		@param language: The language code of the desired recognition language,
			C{None} to use the user's configured language.
		"""
		if language:
			self.language = language
		else:
			self.language = getConfigLanguage()

	def recognize(self, pixels, imgInfo, onResult):
		self._onResult = onResult

		def callback(result):
			# Called on the Rust engine's worker thread once recognition is
			# done. If self._onResult is None, recognition was cancelled.
			if self._onResult:
				if result:
					data = json.loads(result)
					self._onResult(LinesWordsResult(data, imgInfo))
				else:
					self._onResult(RuntimeError("UWP OCR failed"))

		# pixels, as fetched from screenBitmap.captureImage, is a 2d array of
		# RGBQUAD (BGRA) values; pass its raw bytes to the recogniser.
		try:
			nvdaRust.uwp_ocr.recognize(
				self.language,
				bytes(pixels),
				imgInfo.recogWidth,
				imgInfo.recogHeight,
				callback,
			)
		except RuntimeError:
			onResult(RuntimeError("UWP OCR initialization failed"))

	def cancel(self):
		self._onResult = None
