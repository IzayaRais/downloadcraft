#!/usr/bin/env python3
"""Package a native cargo release without downloading build dependencies."""
import argparse, json, pathlib, platform, plistlib, shutil, stat, tempfile, zipfile, re, subprocess
from package_extensions import stage_extensions
p=argparse.ArgumentParser()
p.add_argument('--binary', type=pathlib.Path)
p.add_argument('--platform', choices=['windows','linux','macos'])
p.add_argument('--arch', default=platform.machine())
p.add_argument('--signer', type=pathlib.Path, help='Optional rcodesign binary for cross-built Mac bundles')
p.add_argument('--output', type=pathlib.Path, default=pathlib.Path('dist'))
a=p.parse_args()
repo=pathlib.Path(__file__).resolve().parents[1]
version=(repo/'VERSION').read_text().strip()
osname=a.platform or {'Windows':'windows','Darwin':'macos','Linux':'linux'}[platform.system()]
binary=a.binary or repo/'target/release'/('download_craft.exe' if osname=='windows' else 'download_craft')
a.output.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory() as temp:
    root=pathlib.Path(temp)/f'DownloadCraft-{version}-{osname}-{a.arch}'
    root.mkdir()
    if osname=='macos':
        contents=root/'Download Craft.app/Contents'
        dest=contents/'MacOS/download_craft'
        dest.parent.mkdir(parents=True)
        with (contents/'Info.plist').open('wb') as f:
            plistlib.dump({'CFBundleName':'Download Craft','CFBundleDisplayName':'Download Craft',
                'CFBundleIdentifier':'com.downloadcraft.app','CFBundleExecutable':'download_craft',
                'CFBundlePackageType':'APPL','CFBundleVersion':version.rsplit('.',1)[0]+version.rsplit('.',1)[1] if version.count('.')==3 else version,'CFBundleShortVersionString':version,
                'LSMinimumSystemVersion':'12.0','NSHighResolutionCapable':True},f)
    else:
        dest=root/binary.name
    shutil.copy2(binary,dest)
    if osname!='windows': dest.chmod(0o755)
    stage_extensions(repo,root)
    if osname=='macos':
        resources=contents/'Resources';resources.mkdir(parents=True,exist_ok=True)
        stage_extensions(repo,resources)
    shutil.copy2(repo/'RELEASE-NOTES.md',root/'README.md')
    shutil.copy2(repo/'LICENSE',root/'LICENSE')
    for license_file in (repo/'assets/fonts').glob('OFL-*.txt'):
        shutil.copy2(license_file,root/license_file.name)
    if osname=='macos' and platform.system()=='Darwin':
        subprocess.run(['codesign','--force','--deep','--sign','-',str(contents.parent)],check=True)
    elif osname=='macos' and a.signer:
        subprocess.run([str(a.signer),'sign','--digest','sha256','--timestamp-url','none',str(contents.parent)],check=True)
    archive=a.output/(root.name+'.zip')
    with zipfile.ZipFile(archive,'w',zipfile.ZIP_DEFLATED,compresslevel=9) as z:
        for f in root.rglob('*'):
            if f.is_file(): z.write(f,f.relative_to(root.parent))
    print(archive.resolve())
