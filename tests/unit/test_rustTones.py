# A part of NonVisual Desktop Access (NVDA)
# This file is covered by the GNU General Public License.
# See the file COPYING for more details.

"""Integration tests for the Rust tones module."""

import unittest


class TestRustGenerateBeep(unittest.TestCase):
	"""Test that nvdaRust.tones.generateBeep() works correctly."""

	def test_import(self):
		"""nvdaRust module can be imported."""
		import nvdaRust

		self.assertTrue(hasattr(nvdaRust, "tones"))
		self.assertTrue(hasattr(nvdaRust.tones, "generateBeep"))

	def test_returns_bytes(self):
		"""generateBeep returns a bytes object."""
		import nvdaRust

		result = nvdaRust.tones.generateBeep(440.0, 100, 50, 50)
		self.assertIsInstance(result, bytes)

	def test_buffer_size_440hz_100ms(self):
		"""Buffer size matches expected PCM frame count for 440Hz 100ms."""
		import nvdaRust

		result = nvdaRust.tones.generateBeep(440.0, 100, 50, 50)
		# 4500 samples * 2 channels * 2 bytes = 18000
		self.assertEqual(len(result), 18000)

	def test_buffer_size_1000hz_50ms(self):
		"""Buffer size matches expected PCM frame count for 1000Hz 50ms."""
		import nvdaRust

		result = nvdaRust.tones.generateBeep(1000.0, 50, 50, 50)
		# 2244 samples * 2 channels * 2 bytes = 8976
		self.assertEqual(len(result), 8976)

	def test_stereo_panning_silence_right(self):
		"""With right=0, right channel samples should all be zero."""
		import nvdaRust
		import struct

		result = nvdaRust.tones.generateBeep(440.0, 100, 100, 0)
		# Each frame is 4 bytes: left_i16 + right_i16
		for i in range(0, len(result), 4):
			right_sample = struct.unpack_from("<h", result, i + 2)[0]
			self.assertEqual(right_sample, 0)
