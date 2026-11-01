"""Separate a source glyph touching the rhombus outline; keep the actual rule."""
import json,hashlib
from pathlib import Path
import cv2,numpy as np
from PIL import Image,ImageDraw,ImageFont
D=Path('tmp/pdfs/manual-live-full/live-images')
source_path=next(D.glob('936ac*.source.png'));key=source_path.name.split('.')[0]
source=Image.open(source_path).convert('RGBA');before=Image.open(D/(key+'.selected-preview.png')).convert('RGBA')
a=np.array(source);result=np.array(before);box=(145,488,284,526);x0,y0,x1,y1=box
result[y0:y1,x0:x1]=a[y0:y1,x0:x1]
gray=cv2.cvtColor(a[:,:,:3],cv2.COLOR_RGB2GRAY)
edges=(gray[470:532,90:350]<100).astype('uint8')*255
lines=cv2.HoughLinesP(edges,1,np.pi/720,25,minLineLength=75,maxLineGap=8)
rules=[]
for [[x,y,u,v]] in lines:
 x+=90;u+=90;y+=470;v+=470
 if x==u:continue
 slope=(v-y)/(u-x)
 if .05<abs(slope)<.8:rules.append((slope,y-slope*x))
assert len(rules)>=2,rules
mask=np.zeros(gray.shape,np.uint8)
for y in range(491,523):
 for x in range(149,280):
  rule=max(m*x+c for m,c in rules)
  if abs(y-rule)<2:continue
  if gray[y,x]<235:mask[y,x]=255
clean=cv2.inpaint(a[:,:,:3],mask,4,cv2.INPAINT_TELEA)
result[mask>0,:3]=clean[mask>0]
image=Image.fromarray(result)
target='Gövde açısı sapması';font=ImageFont.truetype('C:/Windows/Fonts/ARIALN.TTF',14*4)
layer=Image.new('RGBA',(136*4,21*4));draw=ImageDraw.Draw(layer);b=font.getbbox(target)
assert font.getlength(target)<layer.width
draw.text(((layer.width-font.getlength(target))/2,2-b[1]),target,font=font,fill=(0,0,0,255))
layer=layer.resize((136,21),Image.Resampling.LANCZOS);image.alpha_composite(layer,(147,504))
changed=np.any(np.array(image)!=np.array(before),axis=2);scope=np.zeros(changed.shape,bool);scope[y0:y1,x0:x1]=True
assert not np.any(changed&~scope)
output=D/(key+'.measured-reviewed.png');image.save(output)
(D/(key+'.measured-review.json')).write_text(json.dumps(dict(method='Source glyph mask with measured rhombus rules excluded, local inpainting and complete Turkish first line',sourcePngSha256=hashlib.sha256(source_path.read_bytes()).hexdigest(),resultSha256=hashlib.sha256(output.read_bytes()).hexdigest(),regions=[dict(source='车身角度偏',translation=target,box=box)],reviewed=False),ensure_ascii=False,indent=2),encoding='utf-8')
print(key,len(rules))
