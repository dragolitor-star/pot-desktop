"""Exact Chinese UI labels read from the supplied manual's clear screenshots.

Used only with high-confidence locally measured OCR in private verification.
Never approximate-match a short UI word or invent a missing source label.
"""
UI_LABELS = {
    '测试结束': 'Test bitti', '测试成功': 'Test başarılı', '测试失败': 'Test başarısız',
    '未测试': 'Test edilmedi', '未知': 'Bilinmiyor', '标定成功': 'Kalibrasyon başarılı',
    '未通过': 'Başarısız', '通过': 'Başarılı', '测试通过': 'Test başarılı',
    '测试中': 'Test ediliyor', '标定中': 'Kalibre ediliyor',
    '基本信息': 'Temel bilgiler', '整机信息': 'Makine bilgileri',
    '能源信息': 'Enerji bilgileri', '固件信息': 'Aygıt yazılımı bilgileri',
    '事件监视器': 'Olay izleyici', '告警监视器': 'Alarm izleyici',
    '任务监视器': 'Görev izleyici', '生产测试': 'Üretim testi',
    '测试组': 'Test grubu', '起始项': 'Başlangıç öğesi', '结束项': 'Bitiş öğesi',
    '停止测试': 'Testi durdur', '开始测试': 'Testi başlat',
    '序列号验证失败': 'Seri numarası doğrulanamadı',
    '请输入产品序列号': 'Ürün seri numarasını girin',
    '校验产品序列号': 'Seri numarasını doğrula', '查看测试结果': 'Test sonuçlarını göster',
    '上传Mes': "Mes'e yükle", '序号': 'Sıra no', '名称': 'Ad',
    '阶段': 'Aşama', '结果': 'Sonuç', '时长(s)': 'Süre (s)',
    '提示信息': 'Durum bilgisi', '提示': 'Bilgi', '取消': 'İptal',
    '确定': 'Tamam', '状态信息': 'Durum bilgisi', '标定': 'Kalibrasyon',
    '时间': 'Zaman', '耗时': 'Süre', '检查设备时间': 'Cihaz saatini denetle',
    '清空': 'Temizle', '文件名称': 'Dosya adı', '文件所在位置': 'Dosya konumu',
    '镜头位置': 'Lens konumu', '上镜头': 'Üst lens', '下镜头': 'Alt lens',
    '菜单': 'Menü', '主页': 'Ana sayfa', '帮助': 'Yardım',
    '说明': 'Not', '注意': 'Dikkat',
    '客户端': 'İstemci', '海康机器人': 'HIKROBOT',
    '双目测距测试': 'Stereo mesafe testi',
    '一体式标定校验': 'Entegre kalibrasyon kontrolü',
}
