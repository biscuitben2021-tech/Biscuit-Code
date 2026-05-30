---
name: format-converter
description: Convert files between mainstream formats — documents (Markdown, DOCX, PDF, HTML, ODT, RTF, EPUB, LaTeX, PPTX), data (CSV, XLSX, JSON, YAML, TOML), images (PNG, JPG, WEBP, GIF, SVG, HEIC, PDF), and audio/video (MP3, WAV, FLAC, MP4, MOV, WEBM, GIF).
triggers:
  - convert
  - conversion
  - to pdf
  - to docx
  - to markdown
  - export as
  - file format
  - pandoc
  - ffmpeg
  - imagemagick
  - csv to
  - json to yaml
  - png to jpg
  - md to docx
tools:
  - Read
  - Bash
  - Write
enabled: true
---

# Format Converter

Use this when the user wants to turn a file from one format into another
("convert this Markdown to DOCX", "make a PDF from this HTML", "PNG → JPG",
"CSV to XLSX", "extract the audio as MP3", and so on).

## Approach

1. **Pin down source → target.** Identify the input format (from its extension;
   confirm with the user if ambiguous) and the exact format the user wants. If
   they didn't name an output path, reuse the input's basename with the new
   extension (e.g. `report.md` → `report.docx`).
2. **Pick the right tool** from the tables below. One tool usually owns a whole
   category: **pandoc** for documents, **LibreOffice** for Office files,
   **ImageMagick** for images, **ffmpeg** for audio/video, **jq/yq/csvkit** for
   data.
3. **Check the tool is installed** (`command -v pandoc`, `command -v ffmpeg`,
   …). If it's missing, tell the user the one-line install (see *Installing
   tools*) and ask before installing — don't silently install heavyweight deps.
4. **Run the conversion** with a clear output path. Quote file paths (they may
   contain spaces).
5. **Verify**: confirm the output file exists and is non-empty (`ls -lh`), and
   for documents sanity-check it (e.g. `pandoc out.docx -t plain | head`). Report
   the output path and any fidelity caveat (see *Caveats*).

> Default to lossless/high-fidelity settings. Never overwrite the source file.

## Documents — `pandoc` (install: `brew install pandoc` / `apt install pandoc`)

Pandoc reads and writes Markdown, HTML, DOCX, ODT, RTF, EPUB, LaTeX, reST,
Textile, MediaWiki, and more, and writes PPTX. Generic form:
`pandoc INPUT -o OUTPUT` (the formats are inferred from extensions).

| From → To | Command |
| --- | --- |
| Markdown → DOCX | `pandoc in.md -o out.docx` |
| Markdown → PDF | `pandoc in.md -o out.pdf` *(needs a PDF engine, below)* |
| Markdown → HTML | `pandoc -s in.md -o out.html` *(`-s` = standalone)* |
| Markdown → PPTX | `pandoc in.md -o out.pptx` |
| Markdown → EPUB | `pandoc in.md -o out.epub` |
| Markdown → LaTeX | `pandoc in.md -o out.tex` |
| DOCX → Markdown | `pandoc in.docx -t gfm -o out.md` |
| DOCX → HTML | `pandoc in.docx -o out.html` |
| HTML → Markdown | `pandoc in.html -t gfm -o out.md` |
| HTML → DOCX | `pandoc in.html -o out.docx` |
| RTF/ODT/EPUB/LaTeX → Markdown | `pandoc in.odt -t gfm -o out.md` |

**PDF output** needs an engine. In order of fidelity/availability:
`pandoc in.md -o out.pdf --pdf-engine=xelatex` (best, needs a TeX dist like
TinyTeX/MacTeX), or `--pdf-engine=weasyprint`, or `--pdf-engine=wkhtmltopdf`.
Embed images with `--resource-path` and add `--toc` for a table of contents.

> Pandoc **cannot read PDF**. For PDF input see the LibreOffice / poppler rows.

## Office files & anything → PDF — LibreOffice (headless)

Best for DOCX/XLSX/PPTX/ODT fidelity and for `* → PDF`. Binary is `libreoffice`
(Linux) or `soffice` (macOS: `/Applications/LibreOffice.app/Contents/MacOS/soffice`).

```sh
libreoffice --headless --convert-to pdf  --outdir ./out  in.docx
libreoffice --headless --convert-to docx --outdir ./out  in.odt
libreoffice --headless --convert-to xlsx --outdir ./out  in.csv
libreoffice --headless --convert-to csv  --outdir ./out  in.xlsx
libreoffice --headless --convert-to pptx --outdir ./out  in.odp
```

PDF → text/editable is inherently lossy. Use poppler:
`pdftotext in.pdf out.txt`, `pdftoppm -png -r 150 in.pdf page` (images per page),
or `libreoffice --headless --convert-to "docx:MS Word 2007 XML" in.pdf` (rough).

## Data & spreadsheets

Prefer portable Python one-liners; `jq`/`yq`/`csvkit` are great if installed.

| From → To | Command |
| --- | --- |
| JSON → YAML | `yq -P -oy in.json > out.yaml` *(mikefarah yq)* — or `python3 -c 'import sys,json,yaml;yaml.safe_dump(json.load(open(sys.argv[1])),sys.stdout,sort_keys=False)' in.json > out.yaml` |
| YAML → JSON | `yq -o=json in.yaml > out.json` — or `python3 -c 'import sys,json,yaml;json.dump(yaml.safe_load(open(sys.argv[1])),sys.stdout,indent=2)' in.yaml > out.json` |
| JSON → TOML | `python3 -c 'import sys,json,tomli_w;tomli_w.dump(json.load(open(sys.argv[1])),open("out.toml","wb"))' in.json` |
| TOML → JSON | `python3 -c 'import sys,json,tomllib;json.dump(tomllib.load(open(sys.argv[1],"rb")),sys.stdout,indent=2)' in.toml` |
| CSV → JSON | `csvjson in.csv > out.json` *(csvkit)* — or pandas: `python3 -c 'import pandas as pd;pd.read_csv("in.csv").to_json("out.json",orient="records",indent=2)'` |
| JSON → CSV | `in2csv -f json in.json > out.csv` — or pandas `pd.read_json(...).to_csv(...)` |
| CSV → XLSX | `libreoffice --headless --convert-to xlsx in.csv` — or `python3 -c 'import pandas as pd;pd.read_csv("in.csv").to_excel("out.xlsx",index=False)'` |
| XLSX → CSV | `libreoffice --headless --convert-to csv in.xlsx` — or pandas `read_excel().to_csv()` |
| Pretty/minify JSON | `jq . in.json` / `jq -c . in.json` |

Install: `brew install jq yq csvkit` / `apt install jq`; `pip install csvkit pandas openpyxl tomli-w pyyaml`.

## Images — ImageMagick (`magick`, IM7) / `convert` (IM6)

Install: `brew install imagemagick` / `apt install imagemagick`.

```sh
magick in.png out.jpg                 # PNG → JPG (any raster ↔ raster)
magick in.png out.webp                # → WEBP   (or: cwebp in.png -o out.webp)
magick in.heic out.jpg                # HEIC → JPG (needs libheif)
magick -density 200 in.pdf out.png    # rasterize PDF page(s) (needs ghostscript)
magick *.jpg out.pdf                  # images → single PDF (or: img2pdf *.jpg -o out.pdf)
rsvg-convert -o out.png in.svg        # SVG → PNG (or: magick / inkscape in.svg out.png)
magick in.png -resize 50% small.png   # resize while converting
```

**macOS without ImageMagick:** `sips -s format jpeg in.heic --out out.jpg`
(`sips` supports png/jpeg/tiff/gif/heic). SVG → PDF/PNG: `rsvg-convert`/`inkscape`.

## Audio & video — `ffmpeg` (install: `brew install ffmpeg` / `apt install ffmpeg`)

```sh
ffmpeg -i in.wav out.mp3              # audio transcode (wav/flac/ogg/m4a/aac/mp3)
ffmpeg -i in.mp3 -c:a flac out.flac   # lossless target
ffmpeg -i in.mov out.mp4              # video transcode (mov/mp4/webm/mkv/avi)
ffmpeg -i in.mp4 -c:v libvpx-vp9 out.webm
ffmpeg -i in.mp4 -vn -q:a 0 out.mp3   # extract audio from video
ffmpeg -i in.mp4 -vf "fps=12,scale=640:-1:flags=lanczos" out.gif   # video → GIF
ffmpeg -i in.gif out.mp4              # GIF → MP4
```

For higher-quality GIFs, generate a palette first
(`ffmpeg -i in.mp4 -vf "fps=12,scale=640:-1,palettegen" pal.png` then
`ffmpeg -i in.mp4 -i pal.png -lavfi "fps=12,scale=640:-1[v];[v][1:v]paletteuse" out.gif`).

## Ebooks — Calibre

`ebook-convert in.epub out.mobi` (also azw3, pdf, docx, txt). Install
`calibre`.

## Archives (quick reference)

`zip -r out.zip dir/` · `unzip in.zip` · `tar -czf out.tar.gz dir/` ·
`tar -xzf in.tar.gz` · `7z a out.7z dir/`.

## Installing tools

| Tool | macOS (Homebrew) | Debian/Ubuntu |
| --- | --- | --- |
| pandoc | `brew install pandoc` | `apt install pandoc` |
| LibreOffice | `brew install --cask libreoffice` | `apt install libreoffice` |
| TeX (for PDF) | `brew install --cask mactex-no-gui` or `tlmgr`/TinyTeX | `apt install texlive-xetex` |
| ImageMagick | `brew install imagemagick` | `apt install imagemagick` |
| ffmpeg | `brew install ffmpeg` | `apt install ffmpeg` |
| poppler (pdftotext/pdftoppm) | `brew install poppler` | `apt install poppler-utils` |
| jq / yq / csvkit | `brew install jq yq csvkit` | `apt install jq` (+ `pip install yq csvkit`) |

## Caveats

- **DOCX/PPTX fidelity**: pandoc is great for content but plain on styling. For
  pixel-faithful Office output, round-trip through LibreOffice. Apply a DOCX
  template with `pandoc --reference-doc=template.docx`.
- **PDF is a dead end for editing**: PDF → DOCX/MD is always lossy (layout, not
  semantics). Extract text with poppler and reflow manually if needed.
- **Color/transparency**: JPG has no alpha — converting PNG→JPG flattens
  transparency (set a background: `magick in.png -background white -flatten out.jpg`).
- **Lossy re-encode**: re-encoding MP3→MP3 or JPG→JPG loses quality each pass;
  convert from the original/lossless source when possible.
- Always confirm the destination and never clobber the input file.
