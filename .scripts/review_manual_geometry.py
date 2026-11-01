"""Measured, source-reviewed geometry overrides for the supplied manual only.

Coordinates below are pixels on the original bitmap, not model coordinates.
Keep this fixture preparation separate from the general production extractor.
"""
import json, math
from pathlib import Path
from PIL import Image

D=Path('tmp/pdfs/manual-live-full/live-images')

def save(prefix, labels):
    source=next(D.glob(prefix+'*.source.png'))
    key=source.name.removesuffix('.source.png')
    w,h=Image.open(source).size
    regions=[]
    for text,translation,(x0,y0,x1,y1) in labels:
        assert 0<=x0<x1<=w and 0<=y0<y1<=h,(text,(w,h))
        regions.append(dict(source_text=text,translation=translation,
            box_2d=[math.floor(y0*1000/h),math.floor(x0*1000/w),math.ceil(y1*1000/h),math.ceil(x1*1000/w)],
            text_color_hex=None,bg_color_hex=None))
    (D/(key+'.manual.regions.json')).write_text(json.dumps(regions,ensure_ascii=False,indent=2),encoding='utf-8')
    (D/(key+'.manual-review.json')).write_text(json.dumps(dict(method='source bitmap pixel measurements',
        width=w,height=h,sourceLabels=[r[0] for r in labels],note='Read and locate glyphs; retain icons, code, frames and arrows.'),ensure_ascii=False,indent=2),encoding='utf-8')
    print(key,len(labels))

save('0b5cb27d',[
 ('起点','Başlangıç',(45,337,84,356)),
 ('后退\n棋盘格采集','Geri git\nDama tahtası verisi al',(149,324,242,368)),
 ('来回走3趟','3 kez ileri geri git',(431,337,520,356)),
 ('执行举升下放货架','Rafı kaldırıp indir',(665,336,818,356)),
])
save('53d20dde',[
 ('任务起点','Görev başlangıcı',(13,53,120,79)),
 ('任务终点','Görev bitişi',(561,58,667,84)),
 ('标识','İşaret',(725,90,768,188)),
])
save('1752618c',[
 ('标定时任务坐标','Kalibrasyon görev koordinatları',(780,220,991,249)),
 ('棋盘格中心拍照','Dama tahtası merkezini çek',(254,114,392,131)),
 ('起点','Başlangıç',(254,174,293,189)),
 ('终点','Bitiş',(254,235,293,250)),
 ('起点','Başlangıç',(254,295,293,310)),
 ('起点','Başlangıç',(254,538,293,553)),
])
# Toolbar labels read from the 743 x 418 source. The hamburger is an icon;
# "三" in the provider transcript is not printed language and is excluded.
save('afc0dca6',[
 ('菜单','Menü',(107,9,124,17)), ('主页','Ana sayfa',(165,9,182,17)),
 ('AMR配置','AMR ayarları',(272,8,306,18)), ('帮助','Yardım',(638,9,653,17)),
 ('快速配置','Hızlı ayar',(163,58,195,66)),
 ('参数对比','Parametre kıyası',(209,58,241,66)),
 ('重定位','Konumlandır',(268,58,286,66)),
 ('遥控控制','Uzaktan kontrol',(307,58,332,66)),
 ('日志下载','Günlük indir',(347,58,375,66)),
 ('升级','Yükselt',(405,37,420,45)),
 ('在线抓包','Çevrimiçi paket yakala',(405,58,433,66)),
 ('激光轮廓','Lazer profili',(472,37,500,45)),
 ('镜头信息','Lens bilgisi',(472,58,500,66)),
 ('RCU监视器','RCU izleyici',(535,58,572,66)),
 ('系统管理','Sistem yönetimi',(603,37,633,45)),
 ('文件管理','Dosya yönetimi',(603,58,633,66)),
 ('默认场景','Varsayılan sahne',(9,38,49,47)),
 ('AMR列表','AMR listesi',(10,64,44,73)),
 ('读取成功','Okundu',(108,120,136,128)),
 ('镜头信息','Lens bilgisi',(175,86,212,95)),
 ('相机类型','Kamera türü',(190,118,219,127)),
 ('读码相机','Kod kamerası',(230,118,260,127)),
 ('上镜头','Üst lens',(464,147,488,156)),
 ('镜头位置','Lens konumu',(464,163,492,172)),
 ('文件名称','Dosya adı',(464,176,492,185)),
 ('文件所在位置','Dosya konumu',(575,194,616,203)),
 ('暂无图像','Görüntü yok',(306,219,336,229)),
 ('下镜头','Alt lens',(464,291,488,300)),
 ('镜头位置','Lens konumu',(464,305,492,315)),
 ('码值','Kod değeri',(464,319,480,328)),
 ('偏移量','Ofset',(464,331,488,341)),
 ('文件名称','Dosya adı',(464,343,492,353)),
 ('文件所在位置','Dosya konumu',(575,362,616,372)),
 ('批量保存','Toplu kaydet',(688,400,722,411)),
])
save('1afa8754',[
 ('菜单','Menü',(198,17,227,31)), ('主页','Ana sayfa',(303,19,334,32)),
 ('AMR配置','AMR ayarları',(497,18,560,33)), ('帮助','Yardım',(1171,16,1200,31)),
 ('快速配置','Hızlı ayar',(301,107,355,121)),
 ('参数对比','Parametre kıyası',(383,107,437,121)),
 ('重定位','Konumlandır',(484,107,524,121)),
 ('遥控控制','Uzaktan kontrol',(559,107,613,121)),
 ('日志下载','Günlük indir',(639,107,694,121)),
 ('升级','Yükselt',(741,71,769,86)),
 ('在线抓包','Çevrimiçi paket yakala',(741,107,795,121)),
 ('激光轮廓','Lazer profili',(866,71,921,86)),
 ('标定','Kalibrasyon',(977,70,1005,86)),
 ('镜头信息','Lens bilgisi',(866,107,921,121)),
 ('RCU监视器','RCU izleyici',(978,107,1046,121)),
 ('系统管理','Sistem yönetimi',(1113,71,1168,86)),
 ('文件管理','Dosya yönetimi',(1113,107,1168,121)),
 ('默认场景','Varsayılan sahne',(15,68,82,87)),
 ('AMR列表','AMR listesi',(17,116,81,132)),
 ('标定','Kalibrasyon',(302,146,335,161)),
 ('标定项列表','Kalibrasyon öğeleri',(294,190,379,207)),
 ('标定结果','Kalibrasyon sonucu',(752,191,818,208)),
 ('导出','Dışa aktar',(1149,192,1177,207)),
 ('一体式标定（激光、相机、里程计）','Entegre kalibrasyon (lazer, kamera, odometre)',(300,360,581,381)),
 ('去标定','Kalibre et',(319,464,357,478)),
 ('时间','Zaman',(766,235,794,249)),
 ('标定项','Kalibrasyon',(904,235,946,249)),
 ('耗时','Süre',(1015,235,1043,249)),
 ('结果','Sonuç',(1075,235,1103,249)),
 ('提示反馈','Geri bildirim',(1155,235,1194,249)),
 ('检查设备时间','Cihaz saatini denetle',(908,270,982,283)),
 ('通过','Başarılı',(1079,270,1104,283)),
 ('测试通过','Test başarılı',(1157,270,1194,283)),
 ('双目标定测试','Stereo kalibrasyon testi',(908,306,982,319)),
 ('未通过','Başarısız',(1079,306,1117,319)),
 ('双目测距测试','Stereo mesafe testi',(908,341,982,355)),
 ('未通过','Başarısız',(1079,341,1117,355)),
 ('双目无障碍测试','Stereo engelsiz ortam testi',(908,377,995,391)),
 ('未通过','Başarısız',(1079,377,1117,391)),
 ('标定过程信息','Kalibrasyon süreç bilgisi',(752,445,852,463)),
 ('清空','Temizle',(1149,448,1177,462)),
 ('下载基础信息','Temel bilgileri indir',(1217,183,1300,198)),
 ('全部复制','Tümünü kopyala',(1247,232,1301,248)),
 ('任务编排','Görev planlama',(321,732,378,747)),
])
