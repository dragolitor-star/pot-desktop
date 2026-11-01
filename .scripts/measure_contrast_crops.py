"""Local second detection pass on the native OCR contrast copy; no network."""
import argparse
import json
from pathlib import Path

import numpy as np
import onnxruntime
from PIL import Image
from rapidocr_onnxruntime import RapidOCR

onnxruntime.disable_telemetry_events()
parser = argparse.ArgumentParser()
parser.add_argument('--prefix', action='append', required=True)
args = parser.parse_args()
directory = Path('tmp/pdfs/manual-live-full/live-images')
engine = RapidOCR(intra_op_num_threads=2, inter_op_num_threads=2, max_side_len=6000,
                  det_limit_side_len=2048, det_limit_type='max')
for source in sorted(directory.glob('*.local-ocr-normalized.png')):
    key = source.name.removesuffix('.local-ocr-normalized.png')
    if not any(key.startswith(prefix) for prefix in args.prefix):
        continue
    target = directory / (key + '.contrast-ocr.json')
    if target.exists():
        continue
    image = Image.open(source).convert('RGB')
    result, _ = engine(np.array(image)[:, :, ::-1], return_word_box=True, text_score=0.0)
    target.write_text(json.dumps(dict(width=image.width, height=image.height, result=result),
                                 ensure_ascii=False), encoding='utf-8')
    print(key, len(result or []), flush=True)
