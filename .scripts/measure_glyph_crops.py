"""Keep low-confidence local text detections for independent crop reading.

Recognition confidence does not decide whether measured pixels contain Chinese;
that classification is reviewed separately. No pixels leave this helper.
"""
import argparse,json
from pathlib import Path
import numpy as np,onnxruntime
from PIL import Image
from rapidocr_onnxruntime import RapidOCR
onnxruntime.disable_telemetry_events()
ap=argparse.ArgumentParser();ap.add_argument('--prefix',action='append',default=[]);ap.add_argument('--all',action='store_true');args=ap.parse_args()
p=Path('tmp/pdfs/manual-live-full');d=p/'live-images';audit=json.loads((p/'local-geometry-audit.json').read_text());flags=json.loads((p/'image-glyph-coverage-flags.json').read_text(encoding='utf-8'))
keys={r['key'] for r in audit if r['missing'] or r['errors']}|{r['key'] for r in flags}
if args.all:
 from review_image_fixtures import current_entries
 keys.update(entry['key'] for entry in current_entries(p))
if args.prefix:keys={k for k in keys if any(k.startswith(prefix) for prefix in args.prefix)}
engine=RapidOCR(intra_op_num_threads=2,inter_op_num_threads=2,max_side_len=6000,det_limit_side_len=2048,det_limit_type='max')
for k in sorted(keys):
 output=d/(k+'.rapidocr-v4-raw.json')
 if output.exists():continue
 im=Image.open(d/(k+'.source.png')).convert('RGBA');rgb=Image.alpha_composite(Image.new('RGBA',im.size,'white'),im).convert('RGB');scale=min(4,max(1,4000//max(im.size)));rgb=rgb.resize((im.width*scale,im.height*scale),Image.Resampling.LANCZOS)
 result,_=engine(np.array(rgb)[:,:,::-1],return_word_box=True,text_score=0.0)
 output.write_text(json.dumps(dict(width=rgb.width,height=rgb.height,result=result),ensure_ascii=False),encoding='utf-8')
 print(k,len(result or []),flush=True)
