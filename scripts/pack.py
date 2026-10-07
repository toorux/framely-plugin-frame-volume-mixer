"""Build a deterministic unsigned Framely package from this plugin's payload."""
import hashlib
import json
import os
import re
import subprocess

import pathlib
import stat
import zipfile

from submit_database import release_url

ROOT = pathlib.Path(__file__).resolve().parents[1]
PAYLOAD_FILES = {'backend', 'page.js', 'icon.png', 'THIRD_PARTY_LICENSES.txt'}


def download_url(manifest, root, repository=None):
    if manifest.get('downloadUrl'):
        return manifest['downloadUrl']
    repository = repository or os.environ.get('GITHUB_REPOSITORY')
    if not repository:
        remote = subprocess.check_output(['git', '-C', str(root), 'remote', 'get-url', 'origin'], text=True).strip()
        match = re.fullmatch(r'(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+?)(?:\.git)?', remote)
        if not match:
            raise ValueError('Cannot infer GitHub repository; set GITHUB_REPOSITORY or downloadUrl')
        repository = match.group(1)
    return release_url(manifest, repository)


def pack(root=ROOT, repository=None):
    root = pathlib.Path(root)
    manifest = json.loads((root / 'manifest.json').read_text())
    manifest['downloadUrl'] = download_url(manifest, root, repository)
    # The whole-package checksum belongs to source registration, never inside its own ZIP.
    manifest.pop('downloadSha256', None)
    payload = root / 'payload'
    paths = list(payload.iterdir())
    if {p.name for p in paths} != PAYLOAD_FILES:
        raise ValueError('Unexpected or missing payload files; rebuild a clean payload')
    for path in paths:
        if not stat.S_ISREG(path.lstat().st_mode):
            raise ValueError('Payload must contain only regular files: ' + path.name)
    files = {p.name: p.read_bytes() for p in sorted(paths)}
    manifest['files'] = {name: hashlib.sha256(data).hexdigest() for name, data in files.items()}
    output = root / 'dist' / f"{manifest['id']}-{manifest['version']}.framely"
    output.parent.mkdir(exist_ok=True)
    raw = json.dumps(manifest, ensure_ascii=False, separators=(',', ':')).encode()
    with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
        for name, data in {'manifest.json': raw, **files}.items():
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data)
    with zipfile.ZipFile(output) as archive:
        if archive.testzip() is not None:
            raise ValueError('Archive CRC verification failed')
        packed = json.loads(archive.read('manifest.json'))
        if set(archive.namelist()) != {'manifest.json', *packed['files']}:
            raise ValueError('Archive file list does not match manifest')
        for name, digest in packed['files'].items():
            if hashlib.sha256(archive.read(name)).hexdigest() != digest:
                raise ValueError('Payload hash mismatch: ' + name)
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    (output.parent / 'SHA256SUMS').write_text(f'{digest}  {output.name}\n')
    print(output)
    return output


if __name__ == '__main__':
    pack()
