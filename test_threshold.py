import fitz

pdf_path = r"C:\Users\enesk\OneDrive\Masaüstü\【0401004】Operation Manual For Parameter Configuration  20260120.pdf"
doc = fitz.open(pdf_path)
page = doc[5] # page 6

# Extract raw character details to simulate the Rust character loop
raw = page.get_text("rawdict")

chars = []
for block in raw["blocks"]:
    if "lines" not in block:
        continue
    for line in block["lines"]:
        for span in line["spans"]:
            for char in span["chars"]:
                chars.append({
                    "c": char["c"],
                    "l": char["bbox"][0],
                    "b": char["bbox"][3],
                    "r": char["bbox"][2],
                    "t": char["bbox"][1],
                })

def simulate(vertical_threshold_factor, add_vertical_split=False):
    result = []
    prev_char = None

    for ch in chars:
        uc = ch["c"]
        l, b, r, t = ch["l"], ch["b"], ch["r"], ch["t"]
        h = t - b
        h_abs = abs(h)
        w = r - l

        if uc == '\n' or uc == '\r':
            result.append(uc)
            prev_char = None
            continue

        if prev_char is not None:
            prev_uc, prev_l, prev_b, prev_r, prev_t = prev_char
            prev_h = abs(prev_t - prev_b)
            current_h = abs(t - b)
            font_height = max(prev_h, current_h, 1.0)

            prev_y_center = (prev_t + prev_b) / 2.0
            y_center = (t + b) / 2.0
            vertical_diff = abs(y_center - prev_y_center)

            # 1. Standard horizontal gap detection on the same visual line
            if vertical_diff < font_height * vertical_threshold_factor:
                horiz_gap = l - prev_r
                threshold = font_height * 1.5
                if horiz_gap > threshold:
                    res_str = "".join(result)
                    if not res_str.endswith("\n\n"):
                        if res_str.endswith("\n"):
                            result.append("\n")
                        else:
                            result.append("\n\n")

            # 2. Add vertical split if we moved down significantly without a newline in the stream
            # In PyMuPDF, Y goes DOWN, so moving down means y_center > prev_y_center.
            elif add_vertical_split and (y_center - prev_y_center) > font_height * 0.8:
                res_str = "".join(result)
                if not res_str.endswith("\n\n"):
                    if res_str.endswith("\n"):
                        result.append("\n")
                    else:
                        result.append("\n\n")

        result.append(uc)
        if h_abs > 0.0 and w > 0.0 and not uc.isspace():
            prev_char = (uc, l, b, r, t)

    return "".join(result)

res_split = simulate(0.85, add_vertical_split=True)
with open("sim_split.txt", "w", encoding="utf-8") as f:
    f.write(res_split)
print("Saved to sim_split.txt")

# Let's run a comparison and write to comparison file
with open("sim_split.txt", "r", encoding="utf-8") as f:
    lines_split = f.read().split("\n")

def get_around(lines, title):
    out = []
    out.append(f"=== {title} ===\n")
    for i, l in enumerate(lines):
        if "不同登录方法的差异" in l or "连接方式" in l or "软AP" in l or "有线" in l or "无线" in l:
            start = max(0, i - 1)
            end = min(len(lines), i + 6)
            for j in range(start, end):
                out.append(f"{j}: {repr(lines[j])}\n")
            out.append("-" * 20 + "\n")
    return "".join(out)

output = get_around(lines_split, "Threshold 0.85 + Vertical Split (Correct Direction)")
with open("sim_comparison2.txt", "w", encoding="utf-8") as f:
    f.write(output)
print("Comparison written to sim_comparison2.txt")
