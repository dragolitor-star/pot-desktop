import fitz # PyMuPDF
pdf_path = r"C:\Users\enesk\OneDrive\Masaüstü\【0401004】Operation Manual For Parameter Configuration  20260120.pdf"

doc = fitz.open(pdf_path)
page = doc[5] # Page 6 (index 5)
drawings = page.get_text("words")
with open("page6_words.txt", "w", encoding="utf-8") as f:
    for w in drawings:
        f.write(f"Word: rect=({w[0]:.1f}, {w[1]:.1f}, {w[2]:.1f}, {w[3]:.1f}) text={w[4]}\n")
print("Words written successfully")
