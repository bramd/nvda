# A part of NonVisual Desktop Access (NVDA)
# Copyright (C) 2025 NV Access Limited
# This file is covered by the GNU General Public License.
# See the file COPYING for more details.

"""App module for Adobe Acrobat and Adobe Reader.

Adobe Acrobat/Reader crashes when the NVDA Visual Highlighter is enabled,
due to Adobe crashing in response to normal accessibility API calls
that NVDA makes to draw highlight rectangles.
See https://github.com/nvaccess/nvda/issues/17834 for details.

As a workaround, the NVDA highlighter is temporarily disabled
while an Adobe Acrobat/Reader window is focused.
"""

import appModuleHandler
import vision
from logHandler import log


#: The provider ID of the NVDA Highlighter vision enhancement provider.
_NVDA_HIGHLIGHTER_PROVIDER_ID = "NVDAHighlighter"


class AppModule(appModuleHandler.AppModule):
	"""App module for Adobe Acrobat and Adobe Reader.
	Disables the NVDA highlighter while this application is focused
	to prevent Adobe from crashing.
	"""

	#: Whether the NVDA highlighter was active before this app module gained focus.
	_wasHighlighterActive: bool = False

	def _getHighlighterProviderInfo(self):
		"""Returns the provider info for the NVDA Highlighter, or None if not found."""
		if not vision.handler:
			return None
		try:
			return vision.handler.getProviderInfo(_NVDA_HIGHLIGHTER_PROVIDER_ID)
		except LookupError:
			return None

	def event_appModule_gainFocus(self):
		"""Temporarily disables the NVDA highlighter to prevent Adobe crashes."""
		providerInfo = self._getHighlighterProviderInfo()
		if providerInfo is None:
			self._wasHighlighterActive = False
			return
		providerInstance = vision.handler.getProviderInstance(providerInfo)
		if providerInstance is None:
			self._wasHighlighterActive = False
			return
		# The highlighter is active; terminate it temporarily.
		self._wasHighlighterActive = True
		try:
			vision.handler.terminateProvider(providerInfo, saveSettings=False)
		except Exception:
			log.debugWarning(
				"Error terminating NVDAHighlighter for Adobe workaround",
				exc_info=True,
			)
			self._wasHighlighterActive = False
			return
		# terminateProvider always calls enableInConfig(False).
		# Restore the config state so the highlighter is still considered enabled
		# and will be re-initialized when leaving Adobe.
		try:
			providerInfo.providerClass.enableInConfig(True)
		except Exception:
			log.debugWarning(
				"Error restoring NVDAHighlighter config state",
				exc_info=True,
			)

	def event_appModule_loseFocus(self):
		"""Re-enables the NVDA highlighter if it was active before Adobe gained focus."""
		if not self._wasHighlighterActive:
			return
		self._wasHighlighterActive = False
		providerInfo = self._getHighlighterProviderInfo()
		if providerInfo is None:
			return
		# Only re-initialize if the provider is still enabled in the configuration.
		if not providerInfo.providerClass.isEnabledInConfig():
			return
		try:
			vision.handler.initializeProvider(providerInfo, temporary=True)
		except Exception:
			log.debugWarning(
				"Error re-initializing NVDAHighlighter after leaving Adobe",
				exc_info=True,
			)
