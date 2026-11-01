"""Native diagnostic renders in isolated fixtures; never publish a cache PNG."""
import argparse, hashlib, json, os, shutil, subprocess
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--kind', choices=['assisted', 'proposal', 'contextual-strong', 'contrast-assisted', 'selected'], default='assisted')
parser.add_argument('--prefix', action='append', default=[])
parser.add_argument('--renderer', default='src-tauri/target/debug/deps/pot-118ceaf5514dd672.exe')
args = parser.parse_args()
directory = Path('tmp/pdfs/manual-live-full/live-images')
scratch = directory.parent / (args.kind + '-renders')
scratch.mkdir(exist_ok=True)
exe = Path(args.renderer).resolve()
renderer_hash = hashlib.sha256(exe.read_bytes()).hexdigest()
env = os.environ.copy()
env['CTRANSLATE_PDFIUM'] = r'C:\Users\enesk\AppData\Roaming\com.pot-app.desktop\pdfium.dll'
selected_keys = None
if args.kind == 'selected':
    selected_keys = {entry['key'] for entry in json.loads((directory.parent / 'selected-image-candidates.json').read_text(encoding='utf-8'))
                     if entry['kind'] not in ['photo', 'measured', 'pending']}
for candidate in sorted(directory.glob('*.' + args.kind + '.regions.json')):
    key = candidate.name.removesuffix('.' + args.kind + '.regions.json')
    if selected_keys is not None and key not in selected_keys:
        continue
    if args.prefix and not any(key.startswith(prefix) for prefix in args.prefix):
        continue
    source = directory / (key + '.source.png')
    target = directory / (key + '.' + args.kind + '-preview.png')
    proof_path = directory / (key + '.' + args.kind + '-render-proof.json')
    source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    regions_bytes = candidate.read_bytes()
    regions_hash = hashlib.sha256(regions_bytes).hexdigest()
    if proof_path.exists() and target.exists():
        prior = json.loads(proof_path.read_text())
        if (prior['sourceSha256'] == source_hash and prior['regionsSha256'] == regions_hash
                and prior['rendererMtime'] == exe.stat().st_mtime
                and prior['previewSha256'] == hashlib.sha256(target.read_bytes()).hexdigest()):
            print(key, 'PASS (unchanged)' if prior['success'] else 'NEEDS REVIEW (unchanged)', flush=True)
            continue
    staged_source = scratch / (key + '.source.png')
    if not staged_source.exists():
        shutil.copy2(source, staged_source)
    staged_regions = scratch / (key + '.regions.json')
    staged_regions.write_bytes(regions_bytes)
    env['CTRANSLATE_IMAGE_FIXTURE'] = str(staged_source.resolve())
    result = subprocess.run([str(exe), 'render_cached_image_regions', '--ignored', '--nocapture'],
                            env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    (directory / (key + '.' + args.kind + '-render.log')).write_bytes(result.stdout)
    preview = scratch / (key + '.preview.png')
    errors = scratch / (key + '.fit-errors.json')
    if preview.exists():
        shutil.copy2(preview, target)
    if errors.exists():
        shutil.copy2(errors, directory / (key + '.' + args.kind + '.fit-errors.json'))
    proof = dict(success=result.returncode == 0, reviewed=False,
                 sourceSha256=source_hash,
                 regionsSha256=hashlib.sha256(staged_regions.read_bytes()).hexdigest(),
                 rendererMtime=exe.stat().st_mtime,
                 rendererSha256=renderer_hash, rendererArchive=str(exe),
                 previewSha256=hashlib.sha256(target.read_bytes()).hexdigest() if preview.exists() else None)
    proof_path.write_text(json.dumps(proof, indent=2))
    print(key, 'PASS' if result.returncode == 0 else 'NEEDS REVIEW', flush=True)
