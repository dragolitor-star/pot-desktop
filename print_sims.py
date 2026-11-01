import sys

with open("sim_04.txt", "r", encoding="utf-8") as f:
    lines_04 = f.read().split("\n")
with open("sim_085.txt", "r", encoding="utf-8") as f:
    lines_085 = f.read().split("\n")

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

output = get_around(lines_04, "Threshold 0.4") + "\n" + get_around(lines_085, "Threshold 0.85")

with open("sim_comparison.txt", "w", encoding="utf-8") as f:
    f.write(output)
print("Comparison written to sim_comparison.txt")
