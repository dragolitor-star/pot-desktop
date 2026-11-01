"""Private source-reviewed fixes for glyphs touching rules and clipped controls."""
import hashlib,json
from pathlib import Path
import cv2,numpy as np
from PIL import Image,ImageDraw,ImageFont
D=Path('tmp/pdfs/manual-live-full/live-images');FONT='C:/Windows/Fonts/ARIALN.TTF'
def load(prefix):
 p=next(D.glob(prefix+'*.source.png'));k=p.name.split('.')[0]
 return k,p,Image.open(p).convert('RGBA'),Image.open(D/(k+'.selected-preview.png')).convert('RGBA')
def label(im,text,box,size=14,rotate=False,color=(0,0,0,255)):
 x0,y0,x1,y1=box;w,h=x1-x0,y1-y0
 if rotate:w,h=h,w
 f=ImageFont.truetype(FONT,size*4);layer=Image.new('RGBA',(w*4,h*4));draw=ImageDraw.Draw(layer)
 lines=text.split('\n');b=f.getbbox('İŞĞgyp');lh=b[3]-b[1]+6
 assert max(f.getlength(line) for line in lines)<=w*4,(text,box)
 assert lh*len(lines)<=h*4,(text,box)
 y=(h*4-lh*len(lines))/2-b[1]
 for line in lines:draw.text(((w*4-f.getlength(line))/2,y),line,font=f,fill=color);y+=lh
 layer=layer.resize((w,h),Image.Resampling.LANCZOS)
 if rotate:layer=layer.transpose(Image.Transpose.ROTATE_270)
 im.alpha_composite(layer,(x0,y0))
def save(k,p,source,result,records,method):
 out=D/(k+'.measured-reviewed.png');result.save(out)
 assert source.size==result.size
 (D/(k+'.measured-review.json')).write_text(json.dumps(dict(method=method,sourcePngSha256=hashlib.sha256(p.read_bytes()).hexdigest(),resultSha256=hashlib.sha256(out.read_bytes()).hexdigest(),regions=records,reviewed=False),ensure_ascii=False,indent=2),encoding='utf-8')
 print(k)

# Four vertical labels beside continuous source reference lines.
k,p,source,result=load('b61b');a=np.array(source);r=np.array(result);records=[]
for left,right in [(40,62),(256,277),(516,538),(733,753)]:
 box=(left,462,right,542);r[455:550,left-10:right]=a[455:550,left-10:right]
 # These source strips contain only the label and transparency. The black
 # continuous line begins at the excluded right boundary.
 r[470:536,left:right]=[0,0,0,0];records.append(dict(source='参考线',translation='Referans hattı',box=box))
result=Image.fromarray(r)
for row in records:label(result,'Referans hattı',row['box'],10,True)
save(k,p,source,result,records,'Four measured transparent glyph strips; excluded continuous rules; fitted rotated type')

# Three rhombi: erase glyph ink only inside their measured source polygon.
k,p,source,result=load('936ac');a=np.array(source);r=np.array(result);records=[]
nodes=[([(63,520),(223,477),(382,520),(223,562)],'Gövde açısı sapması\ngereksinimleri karşılıyor mu?',(112,493,334,548)),
 ([(10,761),(223,718),(435,761),(223,802)],'Gövdenin yanal sapması\ngereksinimleri karşılıyor mu?',(106,733,344,787)),
 ([(647,1015),(790,973),(931,1015),(790,1056)],'Tüm dümen tekerleklerinin\nkalibrasyonu tamamlandı mı?',(681,988,898,1042))]
for polygon,text,box in nodes:
 mask=np.zeros(a.shape[:2],np.uint8);cv2.fillPoly(mask,[np.array(polygon,np.int32)],255)
 inside=cv2.erode(mask,np.ones((3,3),np.uint8))>0
 r[inside]=a[inside]
 gray=cv2.cvtColor(a[:,:,:3],cv2.COLOR_RGB2GRAY);ink=(gray<230)&inside
 # The original fill is uniform (242,242,242); use the observed modal fill.
 fill=np.median(a[inside&(gray>230)],axis=0).astype('uint8');r[ink]=fill
 records.append(dict(translation=text,polygon=polygon,box=box))
result=Image.fromarray(r)
for row in records:label(result,row['translation'],row['box'],16)
save(k,p,source,result,records,'Source polygon interior and observed fill; outlines excluded; complete fitted decision labels')

# Console source commands are immutable outside the inline Chinese value.
k,p,source,result=load('d873');a=np.array(source);r=np.array(result)
r[102:125,:]=a[102:125,:];box=(230,103,276,124)
roi=r[103:124,230:276];ink=roi[:,:,:3].min(2)<240;roi[ink]=[255,255,255,255]
result=Image.fromarray(r);label(result,'Başlatma',box,12)
protected=np.ones(a.shape[:2],bool);protected[103:124,230:276]=False
assert np.array_equal(np.array(result)[102:125][protected[102:125]],a[102:125][protected[102:125]])
save(k,p,source,result,[dict(source='初始化',translation='Başlatma',box=box)],'Inline value fitted inside its measured source bounds; surrounding command pixels verified identical')

# Modal instructions touch red underlines. Preserve every red source pixel.
k,p,source,result=load('09757');a=np.array(source);r=np.array(result);box=(103,318,700,479);x0,y0,x1,y1=box
r[y0:y1,x0:x1]=a[y0:y1,x0:x1];roi=r[y0:y1,x0:x1];rgb=roi[:,:,:3].astype(int)
red=(rgb[:,:,0]>rgb[:,:,1]+40)&(rgb[:,:,0]>rgb[:,:,2]+40)
underlines=np.zeros(red.shape,bool)
for left,top,right,bottom in [(352,387,669,394),(102,424,665,433),(103,468,336,476)]:
 underlines[max(0,top-y0):bottom-y0,max(0,left-x0):right-x0]=True
red &= underlines
ink=(rgb.min(2)<245)&~red;roi[ink]=[255,255,255,255]
result=Image.fromarray(r)
lines=[('x 454 y -27 z 73 konumundaki 0. engel kaçınma stereo kamera testi başlıyor.',(105,319,697,347)),
 ('Kalibrasyon fikstürünü 5 dakika içinde koyun. Yatay stereo kameradan 350 mm uzakta dikey levha kullanın.',(105,359,697,386)),
 ('Aşağı bakan stereo kameranın altına yatay levhayı koyun. Kamera ile kalibrasyon platformu arasındaki',(105,399,697,425)),
 ('dikey yükseklik 300 mm’den büyük, 800 mm’den küçük olmalı. Testi başlatmak için "Tamam"a basın.',(105,439,697,466))]
for text,bounds in lines:label(result,text,bounds,13,color=(125,112,126,255))
r=np.array(result);r[y0:y1,x0:x1][red]=a[y0:y1,x0:x1][red];result=Image.fromarray(r)
assert np.array_equal(np.array(result)[y0:y1,x0:x1][red],a[y0:y1,x0:x1][red])
# Fully visible trailing characters of the repeated statuses below the modal.
r=np.array(result)
for left,right in [(296,361),(366,436)]:
 for top,bottom in [(663,692),(695,736)]:
  r[top:bottom,left:right]=a[top:bottom,left:right]
  roi=r[top:bottom,left:right];bg=a[top:bottom,right+3:right+4];rgb=roi[:,:,:3].astype(int)
  ink=(rgb.max(2)<bg[:,:,:3].max(2)-15)
  for yy in range(bottom-top):roi[yy,ink[yy]]=bg[yy,0]
result=Image.fromarray(r)
for left,right,text in [(296,361,'Test bitti'),(366,436,'Test başarılı')]:
 for top,bottom in [(663,692),(695,736)]:label(result,text,(left,top,right,bottom),12)
save(k,p,source,result,[dict(translation=t,box=b) for t,b in lines],'Measured modal glyph mask; red annotation pixels preserved; complete instructions and repeated statuses')
