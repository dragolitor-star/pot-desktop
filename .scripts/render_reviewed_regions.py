"""Render explicitly measured fixture overrides through the native renderer."""
import os,json,subprocess,shutil
from pathlib import Path
from review_image_fixtures import current_entries
p=Path('tmp/pdfs/manual-live-full/live-images')
current={entry['key'] for entry in current_entries(p.parent)}
exe=Path('src-tauri/target/debug/deps/pot-118ceaf5514dd672.exe').resolve()
env=os.environ.copy();env['CTRANSLATE_PDFIUM']=r'C:\Users\enesk\AppData\Roaming\com.pot-app.desktop\pdfium.dll'
for manual in sorted(p.glob('*.manual.regions.json')):
 k=manual.name.removesuffix('.manual.regions.json')
 if k not in current:continue
 if not (p/(k+'.manual-review.json')).exists():continue
 target=p/(k+'.regions.json');backup=p/(k+'.before-manual.regions.json')
 if not backup.exists():shutil.copy2(target,backup)
 shutil.copy2(manual,target)
 env['CTRANSLATE_IMAGE_FIXTURE']=str((p/(k+'.source.png')).resolve())
 result=subprocess.run([str(exe),'render_cached_image_regions','--ignored','--nocapture'],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 (p/(k+'.render.log')).write_bytes(result.stdout)
 print(k,'PASS' if result.returncode==0 else 'NEEDS FIT REVIEW',flush=True)
info_source=next(p.glob('4194e824*.source.png'))
info=p/(info_source.name.removesuffix('.source.png')+'.regions.json')
regions=json.loads(info.read_text(encoding='utf-8'))
for r in regions:
 if r['source_text']=='说明':r['translation']='Not'
info.write_text(json.dumps(regions,ensure_ascii=False,indent=2),encoding='utf-8')
k=info.name.removesuffix('.regions.json');env['CTRANSLATE_IMAGE_FIXTURE']=str((p/(k+'.source.png')).resolve())
result=subprocess.run([str(exe),'render_cached_image_regions','--ignored','--nocapture'],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
(p/(k+'.render.log')).write_bytes(result.stdout)
print(k,'PASS' if result.returncode==0 else 'NEEDS FIT REVIEW',flush=True)
