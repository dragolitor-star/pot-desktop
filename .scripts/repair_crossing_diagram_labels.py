"""Measured vertical text crossing a bar/rule; restore its known source layer."""
import hashlib,json,math
from pathlib import Path
import numpy as np
from PIL import Image,ImageDraw,ImageFont

D=Path('tmp/pdfs/manual-live-full/live-images')
for prefix in ['53d20d','b61b5a']:
 source_path=next(D.glob(prefix+'*.source.png'));key=source_path.name.split('.')[0]
 source=Image.open(source_path).convert('RGBA');result=Image.open(D/(key+'.selected-preview.png')).convert('RGBA')
 original=np.array(source);array=np.array(result);before=array.copy()
 if prefix=='53d20d':
  box=(720,85,774,195);x0,y0,x1,y1=box
  array[y0:y1,x0:x1]=original[y0:y1,x0:x1]
  rgb=original[y0:y1,x0:x1,:3].astype(int)
  ink=(rgb[:,:,0]>rgb[:,:,1]+2)&(rgb[:,:,0]>rgb[:,:,2]+2)
  import cv2
  ink=cv2.dilate(ink.astype('uint8'),np.ones((3,3),np.uint8))>0
  for yy,xx in zip(*np.where(ink)):
   x,y=x0+xx,y0+yy
   array[y,x]=original[75,x] if 714<=x<730 else [255,255,255,255]
  target='İşaret';font_size=30;color=(255,0,0,255);center=(749,139);plane=(100,45)
 else:
  box=(40,469,62,539);x0,y0,x1,y1=box
  array[y0:y1,x0:x1]=original[y0:y1,x0:x1]
  # The continuous reference line begins at x=62; its pixels are excluded.
  for y in range(470,536):
   for x in range(41,62):
    if original[y,x,3]>0:array[y,x]=[0,0,0,0]
  target='Referans hattı';font_size=10;color=(0,0,0,255);center=(51,503);plane=(66,22)
 result=Image.fromarray(array)
 layer=Image.new('RGBA',(plane[0]*4,plane[1]*4));draw=ImageDraw.Draw(layer)
 font=ImageFont.truetype('C:/Windows/Fonts/ARIALN.TTF',font_size*4)
 bounds=draw.textbbox((0,0),target,font=font)
 assert bounds[2]-bounds[0]<=layer.width and bounds[3]-bounds[1]<=layer.height
 draw.text(((layer.width-font.getlength(target))/2,(layer.height-(bounds[3]-bounds[1]))/2-bounds[1]),target,font=font,fill=color)
 layer=layer.resize(plane,Image.Resampling.LANCZOS).transpose(Image.Transpose.ROTATE_270)
 pos=(round(center[0]-layer.width/2),round(center[1]-layer.height/2));result.alpha_composite(layer,pos)
 changed=np.any(np.array(result)!=before,axis=2);allowed=np.zeros(changed.shape,bool);allowed[y0:y1,x0:x1]=True
 allowed[pos[1]:pos[1]+layer.height,pos[0]:pos[0]+layer.width]|=np.array(layer)[:,:,3]>0
 assert not np.any(changed&~allowed)
 output=D/(key+'.measured-reviewed.png');result.save(output)
 (D/(key+'.measured-review.json')).write_text(json.dumps(dict(method='Source-reviewed vertical glyph mask, source bar/rule profile and rotated fitted type',
  sourcePngSha256=hashlib.sha256(source_path.read_bytes()).hexdigest(),resultSha256=hashlib.sha256(output.read_bytes()).hexdigest(),
  regions=[dict(source='标识' if prefix=='53d20d' else '参考线',translation=target,box=box)],changedPixels=int(changed.sum()),reviewed=False),ensure_ascii=False,indent=2),encoding='utf-8')
 print(key,int(changed.sum()))
