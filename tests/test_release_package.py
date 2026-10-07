import json
import os
import pathlib
import sys
import tempfile
import unittest
import zipfile
from unittest.mock import patch

SCRIPTS = pathlib.Path(__file__).parents[1] / 'scripts'
sys.path.insert(0, str(SCRIPTS))
from pack import pack, download_url, PAYLOAD_FILES
from submit_database import release_url


class ReleasePackage(unittest.TestCase):
    def test_version_only_change_updates_package_and_download_url(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / 'payload').mkdir()
            for name in PAYLOAD_FILES:
                (root / 'payload' / name).write_bytes(b'fixture')
            for version in ('1.0.0', '1.0.1-preview.1'):
                manifest = {'id': 'test.plugin', 'version': version, 'files': {}}
                (root / 'manifest.json').write_text(json.dumps(manifest))
                output = pack(root, repository='author/plugin')
                with zipfile.ZipFile(output) as archive:
                    packed = json.loads(archive.read('manifest.json'))
                self.assertEqual(packed['downloadUrl'],
                    f'https://github.com/author/plugin/releases/download/v{version}/test.plugin-{version}.framely')
                self.assertEqual(json.loads((root / 'manifest.json').read_text()), manifest)
                first = output.read_bytes()
                self.assertEqual(pack(root, repository='author/plugin').read_bytes(), first)

    def test_custom_download_and_hash_config_stays_outside_package(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / 'payload').mkdir()
            for name in PAYLOAD_FILES:
                (root / 'payload' / name).write_bytes(b'fixture')
            manifest = {'id': 'test.plugin', 'version': '1.0.0',
                        'downloadUrl': 'https://example.org/plugin.framely', 'downloadSha256': 'a' * 64}
            (root / 'manifest.json').write_text(json.dumps(manifest))
            with zipfile.ZipFile(pack(root)) as archive:
                packed = json.loads(archive.read('manifest.json'))
            self.assertEqual(packed['downloadUrl'], manifest['downloadUrl'])
            self.assertNotIn('downloadSha256', packed)
            self.assertEqual(json.loads((root / 'manifest.json').read_text()), manifest)

    def test_local_git_remote_and_explicit_url(self):
        manifest = {'id': 'test.plugin', 'version': '1.0.0'}
        with patch.dict(os.environ, {}, clear=True):
            for remote in ('git@github.com:author/plugin.git', 'https://github.com/author/plugin.git', 'ssh://git@github.com/author/plugin'):
                with patch('pack.subprocess.check_output', return_value=remote):
                    self.assertEqual(download_url(manifest, '.'), release_url(manifest, 'author/plugin'))
            explicit = {**manifest, 'downloadUrl': 'https://example.org/plugin.framely'}
            self.assertEqual(download_url(explicit, '.'), explicit['downloadUrl'])


if __name__ == '__main__':
    unittest.main()
