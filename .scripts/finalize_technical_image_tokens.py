"""Final literal-token review, using the source values inspected at full size."""
import json,re
from pathlib import Path
P=Path('tmp/pdfs/manual-live-full');D=P/'live-images'
for e in json.loads((P/'selected-image-candidates.json').read_text(encoding='utf-8')):
 if not e['regions']:continue
 regions=json.loads(Path(e['regions']).read_text(encoding='utf-8'));before=json.dumps(regions,ensure_ascii=False,sort_keys=True);out=[]
 for r in regions:
  if not re.search(r'[\u3400-\u9fff]',r['source_text']):continue
  r['source_text']=re.sub(r'(?<![A-Za-z])PO(?=[\u3400-\u9fff\s]|$)','P0',r['source_text'])
  r['translation']=re.sub(r'(?<![A-Za-z])PO(?=\s|$)','P0',r['translation'])
  if 'ID' in r['source_text']:
   r['translation']=r['translation'].replace('şifreli kimliğe',"şifreli ID'ye")
  if 'WIFI' in r['source_text']:r['translation']=r['translation'].replace('Wi-Fi','WIFI')
  if 'avg_delay' in r['source_text']:r['source_text']=r['source_text'].replace('1ost','lost')
  if e['key'].startswith('49cee') and 'Rbc' in r['source_text']:
   r['source_text']='双目标定成功，Rbc 0.000000 -8.823550 0.000000 度'
   r['translation']='Stereo kamera kalibrasyonu başarılı, Rbc 0.000000 -8.823550 0.000000 derece'
  if e['key'].startswith('a7a79') and 'Rbc' in r['source_text']:
   r['source_text']='双目标定失败，标定结果校验偏差过大，Rbc 0.000000 25.525000 0.000000 度，请检查加密角度和实际安装位姿'
   r['translation']='Stereo kamera kalibrasyonu başarısız; sonuç doğrulama sapması çok büyük. Rbc 0.000000 25.525000 0.000000 derece. Şifreli açıyı ve gerçek montaj pozunu kontrol edin.'
  if e['key'].startswith('e256') and r['source_text'].startswith('45栈板'):r['source_text']=r['source_text'].replace('45栈板','4-5栈板',1)
  out.append(r)
 # A separate source-reviewed recipe protects all surrounding console commands.
 if e['key'].startswith('d873'):continue
 if json.dumps(out,ensure_ascii=False,sort_keys=True)!=before:
  key=e['key'];(D/(key+'.manual.regions.json')).write_text(json.dumps(out,ensure_ascii=False,indent=2),encoding='utf-8')
  review=D/(key+'.manual-review.json');record=json.loads(review.read_text(encoding='utf-8')) if review.exists() else {}
  record['technicalTokenReview']='Literal numeric values checked against the supplied source; P0 confirmed in native source pages 87, 88 and 106. English-only code remains original pixels.'
  review.write_text(json.dumps(record,ensure_ascii=False,indent=2),encoding='utf-8');print(key)
