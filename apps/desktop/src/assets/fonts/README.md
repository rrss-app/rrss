# RRSS Serif 子集

应用本地加载的 Noto Serif SC，SIL Open Font License 1.1，许可原文见 [OFL.txt](OFL.txt)。CSS 家族别名为 `RRSS Serif`，字体内部保留上游名称与版权。无运行时网络请求。

- 字体：Google Fonts 官方仓库 `ofl/notoserifsc/NotoSerifSC[wght].ttf`，固定提交 `8b0a1d0f5983c89bc2b93f1b5fb55f9e252744b5`，字体 name 表版本 `2.003-H1`（hotconv 1.1.1 / makeotfexe 2.6.0）。400、600 从上游可变字体实例化。
- 字表：Unicode 17.0.0 官方 `Unihan.zip` 的 `kTGH` 属性，映射中国 2013 年《通用规范汉字表》，恰好 8105 个码点。加入 ASCII U+0000–007F、通用标点 U+2000–206F、中日韩标点 U+3000–303F、全半角形式 U+FF00–FFEF、NBSP / 间隔号 / 乘除号。请求共 8653 码点；字体实际覆盖 8526（8105 汉字全部覆盖，控制符等无字形码点不会产生字形）。完整码点表见 [subset-unicodes.txt](subset-unicodes.txt)，Unicode 数据许可见 [UNICODE-LICENSE.txt](UNICODE-LICENSE.txt)。
- 工具：Python 3，fonttools 4.60.1，Brotli 1.2.0，zopfli 0.4.3。保留所有布局特性、名称和必要字形。
- 产物：`rrss-serif-400.woff2` 1,603,940 字节（1.53 MiB）；`rrss-serif-600.woff2` 1,625,512 字节（1.55 MiB）。体积低于每份 2–3 MB 的预算。

输入 SHA-256：

```text
NotoSerifSC[wght].ttf  050080d9255a86808f2945bffac582b31ef32bc36411ce29563b4961670c66f9
Unihan.zip            f7a48b2b545acfaa77b2d607ae28747404ce02baefee16396c5d2d7a8ef34b5e
```

在仓库根目录执行以下完整命令重新生成（中间文件放 `/tmp/rrss-serif-build`）：

```bash
mkdir -p /tmp/rrss-serif-build
python3 -m venv /tmp/rrss-serif-build/venv
/tmp/rrss-serif-build/venv/bin/pip install fonttools==4.60.1 brotli==1.2.0 zopfli==0.4.3
curl -L --fail --globoff 'https://raw.githubusercontent.com/google/fonts/8b0a1d0f5983c89bc2b93f1b5fb55f9e252744b5/ofl/notoserifsc/NotoSerifSC[wght].ttf' -o /tmp/rrss-serif-build/NotoSerifSC.ttf
curl -L --fail 'https://raw.githubusercontent.com/google/fonts/8b0a1d0f5983c89bc2b93f1b5fb55f9e252744b5/ofl/notoserifsc/OFL.txt' -o apps/desktop/src/assets/fonts/OFL.txt
curl -L --fail 'https://www.unicode.org/license.txt' -o apps/desktop/src/assets/fonts/UNICODE-LICENSE.txt
curl -L --fail 'https://www.unicode.org/Public/17.0.0/ucd/Unihan.zip' -o /tmp/rrss-serif-build/Unihan.zip
/tmp/rrss-serif-build/venv/bin/python - <<'PY'
from pathlib import Path
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
import hashlib
import subprocess
import zipfile

work = Path('/tmp/rrss-serif-build')
out = Path('apps/desktop/src/assets/fonts')
assert hashlib.sha256((work / 'NotoSerifSC.ttf').read_bytes()).hexdigest() == '050080d9255a86808f2945bffac582b31ef32bc36411ce29563b4961670c66f9'
assert hashlib.sha256((work / 'Unihan.zip').read_bytes()).hexdigest() == 'f7a48b2b545acfaa77b2d607ae28747404ce02baefee16396c5d2d7a8ef34b5e'
z = zipfile.ZipFile(work / 'Unihan.zip')
han = {int(line.split('\t')[0][2:], 16)
       for name in z.namelist() for line in z.read(name).decode().splitlines()
       if '\tkTGH\t' in line}
assert len(han) == 8105
extra = (set(range(0x80)) | set(range(0x2000, 0x2070))
         | set(range(0x3000, 0x3040)) | set(range(0xff00, 0xfff0))
         | {0xa0, 0xb7, 0xd7, 0xf7})
(out / 'subset-unicodes.txt').write_text(','.join(f'U+{cp:04X}' for cp in sorted(han | extra)) + '\n')
for weight in (400, 600):
    font = TTFont(work / 'NotoSerifSC.ttf')
    static = work / f'serif-{weight}.ttf'
    instantiateVariableFont(font, {'wght': weight}, inplace=True).save(static)
    target = out / f'rrss-serif-{weight}.woff2'
    subprocess.run([str(work / 'venv/bin/pyftsubset'), str(static),
                    f'--unicodes-file={out}/subset-unicodes.txt', '--flavor=woff2',
                    f'--output-file={target}', '--layout-features=*', '--name-IDs=*',
                    '--name-languages=*', '--notdef-glyph', '--notdef-outline',
                    '--recommended-glyphs'], check=True)
    result = TTFont(target)
    assert han <= result.getBestCmap().keys()
    assert result['OS/2'].usWeightClass == weight
    print(target, target.stat().st_size)
PY
```

WOFF2 内含字体时间戳，重建时字节散列可能不同；验收以字重、完整 8105 汉字覆盖和文件大小为准。Vite 会将 CSS 中的相对字体路径打包到 `dist/assets/`，Tauri 现有 `font-src 'self'` 允许本地字体加载。
