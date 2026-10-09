import os
from pathlib import Path
import subprocess
import unittest


class DiskImageCompatibilityTests(unittest.TestCase):
    def test_folder_packaging_requires_the_volume_name_option(self):
        script = Path(__file__).with_name("disk_image.sh")
        for help_text, exit_code, expected in [
            ("create from --format <format> <disk> <destination>", 0, "hdiutil"),
            ("create from --volumeName <name> <folder> <destination>", 0, "diskutil"),
            ("unknown command", 1, "hdiutil"),
        ]:
            with self.subTest(help_text=help_text):
                env = os.environ.copy()
                env.pop("DISK_IMAGE_TOOL", None)
                env["KM003C_TEST_DISK_HELP"] = help_text
                env["KM003C_TEST_DISK_EXIT"] = str(exit_code)
                result = subprocess.run(
                    ["bash", "-c", '''
diskutil() { printf '%s\n' "$KM003C_TEST_DISK_HELP"; return "$KM003C_TEST_DISK_EXIT"; }
source "$1"
printf '%s' "$DISK_IMAGE_TOOL"
''', "disk-image-test", str(script)],
                    env=env, capture_output=True, text=True, check=True,
                )
                self.assertEqual(result.stdout, expected)


if __name__ == "__main__":
    unittest.main()
