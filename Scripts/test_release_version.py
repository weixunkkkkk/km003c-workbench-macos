import unittest

from validate_release_version import validate_tag


class ReleaseVersionTests(unittest.TestCase):
    def test_application_tags_and_manual_build(self):
        for tag in ("", "v0.1.0", "v0.1.0-20260908", "v0.1.0-20240229"):
            with self.subTest(tag=tag):
                validate_tag(tag, "0.1.0")

    def test_rejects_wrong_version_invalid_dates_and_suffixes(self):
        for tag in ("v0.3.0", "v0.2.0-20260908", "v0.1.0-20260229", "v0.1.0-20261301",
                    "v0.1.0-2026098", "v0.1.0-20260908-extra", "0.1.0", "v0.1.0\n"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                validate_tag(tag, "0.1.0")

    def test_rejects_invalid_application_version(self):
        with self.assertRaises(ValueError):
            validate_tag("", "")


if __name__ == "__main__":
    unittest.main()
