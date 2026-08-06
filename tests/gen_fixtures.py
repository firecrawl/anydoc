#!/usr/bin/env python3
"""Regenerate the committed fixture corpus in tests/fixtures/.

Sources live in tests/fixture-src/. Real-producer files are generated through
LibreOffice (headless) and Pandoc; `handmade-*` files are assembled directly for
precise edge cases. Malformed fixtures encode their expected outcome under the
unified recovery policy in the filename: `name--recovers.ext`, `name--skips.ext`,
`name--ignores.ext`, `name--errors.ext`. Files under abuse/ are resource-abuse
shapes and are excluded from the baseline snapshot run.

Usage: python tests/gen_fixtures.py [--skip-office]
"""

import base64
import os
import re
import shutil
import struct
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "tests" / "fixture-src"
OUT = ROOT / "tests" / "fixtures"
SOFFICE = r"C:\Program Files\LibreOffice\program\soffice.exe"
ZIP_DATE = (2026, 1, 1, 0, 0, 0)

DOT_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8"
    "z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
)


def run(cmd, **kw):
    print("+", " ".join(str(c) for c in cmd))
    subprocess.run(cmd, check=True, **kw)


def convert_lo(src, filter_spec, out_dir, final_name):
    out_dir.mkdir(parents=True, exist_ok=True)
    profile = ROOT / "target" / "lo-profile"
    ext = filter_spec.split(":")[0]
    produced = out_dir / (src.stem + "." + ext)
    # The very first soffice launch may only initialize its profile; retry.
    for attempt in range(3):
        run([
            SOFFICE, "--headless", "--norestore",
            f"-env:UserInstallation={profile.as_uri()}",
            "--convert-to", filter_spec, "--outdir", str(out_dir), str(src),
        ])
        if produced.exists():
            break
    else:
        raise RuntimeError(f"soffice produced no output for {src} -> {filter_spec}")
    target = out_dir / final_name
    if produced != target:
        if target.exists():
            target.unlink()
        produced.rename(target)
    if ext == "rtf":
        anchor_local_paths(target)


# LibreOffice resolves a relative hyperlink against the source file's own
# location on RTF export, so the generating machine's absolute path lands in
# the fixture. Anchor it to a fixed root: the fixture tests that a `HYPERLINK`
# field carrying a file URL is read, not which directory generated it.
FIXTURE_URI_ROOT = b"file:///anydoc"


def anchor_local_paths(path):
    data = path.read_bytes()
    anchored = re.sub(rb'file:///[^"]*?/tests/fixture-src/',
                      FIXTURE_URI_ROOT + b"/tests/fixture-src/", data)
    if anchored != data:
        path.write_bytes(anchored)


def write_cfb(path, streams):
    """Minimal CFB v3 writer (512-byte sectors) for handmade OLE2 fixtures.

    Streams are padded to the 4096-byte mini-stream cutoff so everything
    lives in regular FAT sectors; no mini-FAT is written. `streams` is a
    list of (name, bytes) in CFB name order (shorter names first, then
    case-insensitive).
    """
    SECT = 512
    ENDOFCHAIN = 0xFFFFFFFE
    FATSECT = 0xFFFFFFFD
    FREESECT = 0xFFFFFFFF

    fat = []          # sector id -> next sector id
    sectors = []      # sector payloads, id == index

    def add_chain(data):
        if not data:
            return ENDOFCHAIN
        padded = data + b"\x00" * (-len(data) % SECT)
        first = len(sectors)
        count = len(padded) // SECT
        for i in range(count):
            sectors.append(padded[i * SECT:(i + 1) * SECT])
            fat.append(first + i + 1 if i + 1 < count else ENDOFCHAIN)
        return first

    def dir_entry(name, typ, color, left, right, child, start, size):
        raw = name.encode("utf-16-le") + b"\x00\x00"
        return (raw + b"\x00" * (64 - len(raw))
                + struct.pack("<HBBIII", len(raw), typ, color, left, right, child)
                + b"\x00" * 16          # clsid
                + b"\x00" * 4           # state bits
                + b"\x00" * 16          # ctime/mtime
                + struct.pack("<IQ", start, size))

    NONE = 0xFFFFFFFF
    entries = [None]  # placeholder for root
    starts = []
    for name, data in streams:
        padded = data + b"\x00" * (4096 - len(data)) if 0 < len(data) < 4096 else data
        starts.append((add_chain(padded), len(padded)))
    for i, (name, _) in enumerate(streams):
        sid = i + 1
        right = sid + 1 if sid < len(streams) else NONE
        start, size = starts[i]
        entries.append(dir_entry(name, 2, 1, NONE, right, NONE, start, size))
    child = 1 if streams else NONE
    entries[0] = dir_entry("Root Entry", 5, 1, NONE, NONE, child, ENDOFCHAIN, 0)

    dir_data = b"".join(entries)
    dir_data += b"\x00" * (-len(dir_data) % SECT)
    dir_start = add_chain(dir_data)

    # FAT sectors (each holds 128 entries), marked FATSECT in the FAT itself.
    fat_count = 1
    while (len(fat) + fat_count + 127) // 128 > fat_count:
        fat_count += 1
    fat_start = len(sectors)
    fat_ids = list(range(fat_start, fat_start + fat_count))
    full_fat = fat + [FATSECT] * fat_count
    full_fat += [FREESECT] * (fat_count * 128 - len(full_fat))
    for i in range(fat_count):
        sectors.append(struct.pack("<128I", *full_fat[i * 128:(i + 1) * 128]))

    difat = fat_ids + [FREESECT] * (109 - len(fat_ids))
    header = (b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" + b"\x00" * 16
              + struct.pack("<HHHHHHIIIIIIIII",
                            0x003E, 0x0003, 0xFFFE, 9, 6, 0, 0, 0,
                            fat_count, dir_start, 0, 4096,
                            ENDOFCHAIN, 0, ENDOFCHAIN)
              + struct.pack("<I", 0)
              + struct.pack("<109I", *difat))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(header + b"".join(sectors))


def write_zip(path, entries, mimetype_first=None):
    """entries: list of (name, bytes). Deterministic timestamps."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        if mimetype_first is not None:
            info = zipfile.ZipInfo("mimetype", date_time=ZIP_DATE)
            z.writestr(info, mimetype_first, compress_type=zipfile.ZIP_STORED)
        for name, data in entries:
            info = zipfile.ZipInfo(name, date_time=ZIP_DATE)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)


# ---------------------------------------------------------------------------
# Handmade DOCX: complete numbering matrix + style inheritance (P7.4a-f, H6, M1)

W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
R = 'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'

CONTENT_TYPES_BASE = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
{extra}</Types>"""

ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"""


def para(text, ppr=""):
    return f"<w:p>{ppr}<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>"


def numpara(text, num_id, ilvl=0, pstyle=None):
    ps = f'<w:pStyle w:val="{pstyle}"/>' if pstyle else ""
    ppr = (f"<w:pPr>{ps}<w:numPr><w:ilvl w:val=\"{ilvl}\"/>"
           f"<w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr>")
    return para(text, ppr)


def numbering_docx():
    styles = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles {W}>
<w:docDefaults><w:rPrDefault><w:rPr/></w:rPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:styleId="HeadBase">
  <w:name w:val="Head Base"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr>
</w:style>
<w:style w:type="paragraph" w:styleId="SubHead">
  <w:name w:val="Sub Head"/><w:basedOn w:val="HeadBase"/>
  <w:rPr><w:b/></w:rPr>
</w:style>
<w:style w:type="paragraph" w:styleId="NumberedPara">
  <w:name w:val="Numbered Para"/>
  <w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr>
</w:style>
<w:style w:type="paragraph" w:styleId="BoldPara">
  <w:name w:val="Bold Para"/><w:rPr><w:b/></w:rPr>
</w:style>
<w:style w:type="character" w:styleId="PlainChar">
  <w:name w:val="Plain Char"/><w:rPr><w:b w:val="0"/></w:rPr>
</w:style>
<w:style w:type="character" w:styleId="ToggleChar">
  <w:name w:val="Toggle Char"/><w:rPr><w:b/></w:rPr>
</w:style>
<w:style w:type="paragraph" w:styleId="BoldBase">
  <w:name w:val="Bold Base"/><w:rPr><w:b/></w:rPr>
</w:style>
<w:style w:type="paragraph" w:styleId="DoubleBold">
  <w:name w:val="Double Bold"/><w:basedOn w:val="BoldBase"/>
  <w:rPr><w:b w:val="true"/></w:rPr>
</w:style>
<w:style w:type="numbering" w:styleId="ListNumStyle">
  <w:name w:val="List Num Style"/>
  <w:pPr><w:numPr><w:numId w:val="4"/></w:numPr></w:pPr>
</w:style>
<w:style w:type="paragraph" w:styleId="ListLevelOne">
  <w:name w:val="List Level One"/>
  <w:pPr><w:numPr><w:ilvl w:val="5"/><w:numId w:val="7"/></w:numPr></w:pPr>
</w:style>
<w:style w:type="paragraph" w:styleId="ListLevelTwo">
  <w:name w:val="List Level Two"/>
  <w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="7"/></w:numPr></w:pPr>
</w:style>
</w:styles>"""

    numbering = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering {W}>
<w:abstractNum w:abstractNumId="0">
  <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>
  <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerRoman"/><w:lvlText w:val="%2)"/><w:lvlRestart w:val="0"/></w:lvl>
  <w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="&#8226;"/></w:lvl>
</w:abstractNum>
<w:abstractNum w:abstractNumId="1">
  <w:numStyleLink w:val="ListNumStyle"/>
</w:abstractNum>
<w:abstractNum w:abstractNumId="2">
  <w:styleLink w:val="ListNumStyle"/>
  <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="upperLetter"/><w:lvlText w:val="%1:"/></w:lvl>
</w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="2"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="3"><w:abstractNumId w:val="0"/>
  <w:lvlOverride w:ilvl="0"><w:startOverride w:val="10"/></w:lvlOverride>
</w:num>
<w:num w:numId="4"><w:abstractNumId w:val="2"/></w:num>
<w:num w:numId="5"><w:abstractNumId w:val="1"/></w:num>
<w:num w:numId="6"><w:abstractNumId w:val="0"/>
  <w:lvlOverride w:ilvl="0">
    <w:startOverride w:val="7"/>
    <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="upperLetter"/><w:lvlText w:val="%1)"/></w:lvl>
  </w:lvlOverride>
</w:num>
<w:abstractNum w:abstractNumId="3">
  <w:lvl w:ilvl="0"><w:pStyle w:val="ListLevelOne"/><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>
  <w:lvl w:ilvl="1"><w:pStyle w:val="ListLevelTwo"/><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%2."/></w:lvl>
</w:abstractNum>
<w:num w:numId="7"><w:abstractNumId w:val="3"/></w:num>
</w:numbering>"""

    body = "".join([
        numpara("One-one", 1),
        numpara("One-two", 1),
        numpara("One-two-a roman", 1, 1),
        numpara("One-two-b roman", 1, 1),
        numpara("Deep bullet", 1, 2),
        numpara("One-three", 1),
        para("Interruption paragraph."),
        numpara("One-four continues the count", 1),
        numpara("Two-one independent counter", 2),
        numpara("Ten-start via override", 3),
        numpara("Letter-A via numStyleLink", 5),
        numpara("Level replaced and start overridden together", 6),
        # suppressed numbering: style brings numId 1, direct numPr zeroes it
        numpara("Suppressed numbering paragraph", 0, 0, pstyle="NumberedPara"),
        # style-provided numbering only (no direct numPr)
        para("Style-numbered paragraph", '<w:pPr><w:pStyle w:val="NumberedPara"/></w:pPr>'),
        # S3: style-based numbering resolves its level through the abstract
        # levels' pStyle bindings; the ilvl inside the styles' numPr (5 and
        # 0 above - both wrong on purpose) must be ignored.
        para("pStyle-bound level one", '<w:pPr><w:pStyle w:val="ListLevelOne"/></w:pPr>'),
        para("pStyle-bound level two", '<w:pPr><w:pStyle w:val="ListLevelTwo"/></w:pPr>'),
        para("pStyle-bound level one again", '<w:pPr><w:pStyle w:val="ListLevelOne"/></w:pPr>'),
        # heading level via basedOn-inherited outlineLvl
        para("Inherited subheading", '<w:pPr><w:pStyle w:val="SubHead"/></w:pPr>'),
        # Toggle semantics (ECMA-376 17.7.3):
        # (a) style-level false is a no-op -> stays bold
        ('<w:p><w:pPr><w:pStyle w:val="BoldPara"/></w:pPr>'
         '<w:r><w:t xml:space="preserve">Bold here, </w:t></w:r>'
         '<w:r><w:rPr><w:rStyle w:val="PlainChar"/></w:rPr>'
         '<w:t xml:space="preserve">and style-false keeps it bold</w:t></w:r></w:p>'),
        # (b) character-style true toggles inherited bold off
        ('<w:p><w:pPr><w:pStyle w:val="BoldPara"/></w:pPr>'
         '<w:r><w:rPr><w:rStyle w:val="ToggleChar"/></w:rPr>'
         '<w:t xml:space="preserve">toggled off by a true in the character style</w:t>'
         '</w:r></w:p>'),
        # (c) two trues along one basedOn chain cancel out
        ('<w:p><w:pPr><w:pStyle w:val="DoubleBold"/></w:pPr>'
         '<w:r><w:t xml:space="preserve">double toggle cancels to plain</w:t></w:r></w:p>'),
        # (d) direct formatting is absolute; "off" is a valid ST_OnOff value
        ('<w:p><w:pPr><w:pStyle w:val="BoldPara"/></w:pPr>'
         '<w:r><w:rPr><w:b w:val="off"/></w:rPr>'
         '<w:t xml:space="preserve">direct off wins absolutely</w:t></w:r></w:p>'),
        # (e) direct "on" is a valid ST_OnOff value
        ('<w:p><w:r><w:rPr><w:i w:val="on"/></w:rPr>'
         '<w:t xml:space="preserve">direct on makes this italic</w:t></w:r></w:p>'),
    ])
    document = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                f'<w:document {W} {R}><w:body>{body}</w:body></w:document>')
    doc_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>
</Relationships>"""
    ct = CONTENT_TYPES_BASE.format(extra=(
        '<Override PartName="/word/numbering.xml" ContentType='
        '"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>'))
    write_zip(OUT / "docx" / "handmade-numbering.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", ROOT_RELS),
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", doc_rels),
        ("word/styles.xml", styles),
        ("word/numbering.xml", numbering),
    ])


# ---------------------------------------------------------------------------
# Handmade DOCX: chart + SmartArt + image rel (P7.5, P7.10, M4)

def rich_docx():
    chart = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<c:chart>
<c:title><c:tx><c:rich><a:p><a:r><a:t>Quarterly Widgets</a:t></a:r></a:p></c:rich></c:tx></c:title>
<c:plotArea>
<c:barChart>
<c:ser>
<c:tx><c:strRef><c:f>S</c:f><c:strCache><c:pt c:idx="0"><c:v>Widgets</c:v></c:pt></c:strCache></c:strRef></c:tx>
<c:cat><c:strRef><c:f>C</c:f><c:strCache>
<c:pt c:idx="0"><c:v>Q1</c:v></c:pt><c:pt c:idx="1"><c:v>Q2</c:v></c:pt>
</c:strCache></c:strRef></c:cat>
<c:val><c:numRef><c:f>V</c:f><c:numCache>
<c:pt c:idx="0"><c:v>10</c:v></c:pt><c:pt c:idx="1"><c:v>14</c:v></c:pt>
</c:numCache></c:numRef></c:val>
</c:ser>
</c:barChart>
<c:catAx><c:title><c:tx><c:rich><a:p><a:r><a:t>Quarter</a:t></a:r></a:p></c:rich></c:tx></c:title></c:catAx>
<c:valAx><c:title><c:tx><c:rich><a:p><a:r><a:t>Units</a:t></a:r></a:p></c:rich></c:tx></c:title></c:valAx>
</c:plotArea>
</c:chart>
</c:chartSpace>"""

    diagram_data = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<dgm:ptLst>
<dgm:pt dgm:modelId="1" dgm:type="doc"/>
<dgm:pt dgm:modelId="2"><dgm:t><a:bodyPr/><a:p><a:r><a:t>Plan</a:t></a:r></a:p></dgm:t></dgm:pt>
<dgm:pt dgm:modelId="3"><dgm:t><a:bodyPr/><a:p><a:r><a:t>Build</a:t></a:r></a:p></dgm:t></dgm:pt>
<dgm:pt dgm:modelId="4"><dgm:t><a:bodyPr/><a:p><a:r><a:t>Ship</a:t></a:r></a:p></dgm:t></dgm:pt>
</dgm:ptLst>
</dgm:dataModel>"""

    drawing_chart = (
        '<w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">'
        '<wp:extent cx="3000000" cy="2000000"/><wp:docPr id="1" name="Chart 1" descr="widget chart"/>'
        '<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
        '<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">'
        '<c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rId10"/>'
        "</a:graphicData></a:graphic></wp:inline></w:drawing>"
    )
    drawing_diagram = (
        '<w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">'
        '<wp:extent cx="3000000" cy="2000000"/><wp:docPr id="2" name="Diagram 1"/>'
        '<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
        '<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram">'
        '<dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rId20" r:lo="" r:qs="" r:cs=""/>'
        "</a:graphicData></a:graphic></wp:inline></w:drawing>"
    )
    drawing_image = (
        '<w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">'
        '<wp:extent cx="100000" cy="100000"/><wp:docPr id="3" name="Dot" descr="tiny dot image"/>'
        '<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
        '<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        '<pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        '<pic:blipFill><a:blip r:embed="rId30"/></pic:blipFill></pic:pic>'
        "</a:graphicData></a:graphic></wp:inline></w:drawing>"
    )

    drawing_ole = (
        '<w:object xmlns:o="urn:schemas-microsoft-com:office:office" '
        'xmlns:v="urn:schemas-microsoft-com:vml">'
        '<v:shape id="ole1" style="width:100pt;height:50pt"/>'
        '<o:OLEObject Type="Embed" ProgID="Excel.Sheet.12" ShapeID="ole1" r:id="rId40"/>'
        "</w:object>"
    )

    body = (
        para("Rich objects follow.")
        + f"<w:p><w:r>{drawing_chart}</w:r></w:p>"
        + f"<w:p><w:r>{drawing_diagram}</w:r></w:p>"
        + f"<w:p><w:r>{drawing_image}</w:r></w:p>"
        + f"<w:p><w:r>{drawing_ole}</w:r></w:p>"
        + para("After the objects.")
    )
    document = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                f'<w:document {W} {R}><w:body>{body}</w:body></w:document>')
    doc_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
<Relationship Id="rId10" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/chart1.xml"/>
<Relationship Id="rId20" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData" Target="diagrams/data1.xml"/>
<Relationship Id="rId30" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/dot.png"/>
<Relationship Id="rId40" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject" Target="embeddings/oleObject1.bin"/>
</Relationships>"""
    styles = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
              f'<w:styles {W}><w:docDefaults><w:rPrDefault><w:rPr/></w:rPrDefault></w:docDefaults></w:styles>')
    ct = CONTENT_TYPES_BASE.format(extra=(
        '<Default Extension="png" ContentType="image/png"/>'
        '<Default Extension="bin" ContentType="application/vnd.openxmlformats-officedocument.oleObject"/>'
        '<Override PartName="/word/charts/chart1.xml" ContentType='
        '"application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/>'
        '<Override PartName="/word/diagrams/data1.xml" ContentType='
        '"application/vnd.openxmlformats-officedocument.drawingml.diagramData+xml"/>'))
    write_zip(OUT / "docx" / "handmade-rich.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", ROOT_RELS),
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", doc_rels),
        ("word/styles.xml", styles),
        ("word/charts/chart1.xml", chart),
        ("word/diagrams/data1.xml", diagram_data),
        ("word/media/dot.png", DOT_PNG),
        ("word/embeddings/oleObject1.bin", b"OLE-PAYLOAD-STAND-IN" * 4),
    ])


# ---------------------------------------------------------------------------
# Handmade PPTX: placeholder inheritance through layout and master (H7)

PPTX_NS = ('xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
           'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" '
           'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"')


def inherit_pptx():
    presentation = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation {PPTX_NS}>
<p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>
</p:presentation>"""
    pres_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"""
    # Master: body text is bold with a character bullet at level 1.
    master = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster {PPTX_NS}>
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
</p:spTree></p:cSld>
<p:txStyles>
<p:titleStyle><a:lvl1pPr><a:buNone/><a:defRPr/></a:lvl1pPr></p:titleStyle>
<p:bodyStyle><a:lvl1pPr><a:buChar char="•"/><a:defRPr b="1"/></a:lvl1pPr>
<a:lvl2pPr><a:buChar char="-"/><a:defRPr i="1"/></a:lvl2pPr></p:bodyStyle>
<p:otherStyle/>
</p:txStyles>
</p:sldMaster>"""
    # Layout: overrides the body placeholder's level-1 bullet to roman
    # numbering starting at 3.
    layout = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout {PPTX_NS}>
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
<p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Layout title text</a:t></a:r></a:p></p:txBody></p:sp>
<p:sp><p:nvSpPr><p:cNvPr id="3" name="Body"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr>
<p:spPr/><p:txBody><a:bodyPr/>
<a:lstStyle><a:lvl1pPr><a:buAutoNum type="romanLcPeriod" startAt="3"/></a:lvl1pPr></a:lstStyle>
<a:p><a:endParaRPr/></a:p></p:txBody></p:sp>
</p:spTree></p:cSld>
</p:sldLayout>"""
    slide = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {PPTX_NS}>
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="ctrTitle"/></p:nvPr></p:nvSpPr>
<p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Inherited Title Slide</a:t></a:r></a:p></p:txBody></p:sp>
<p:sp><p:nvSpPr><p:cNvPr id="3" name="Content"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr>
<p:spPr/><p:txBody><a:bodyPr/>
<a:p><a:r><a:t>Roman three bold via master</a:t></a:r></a:p>
<a:p><a:r><a:t>Roman four </a:t></a:r><a:r><a:rPr b="0"/><a:t>with bold turned off</a:t></a:r></a:p>
<a:p><a:pPr lvl="1"/><a:r><a:t>Dash bullet italic via master level two</a:t></a:r></a:p>
<a:p><a:pPr><a:buNone/></a:pPr><a:r><a:t>Plain closing line</a:t></a:r></a:p>
</p:txBody></p:sp>
</p:spTree></p:cSld>
</p:sld>"""
    slide_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
</Relationships>"""
    layout_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>
</Relationships>"""
    ct = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
<Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
<Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>
<Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>
</Types>"""
    root_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"""
    write_zip(OUT / "pptx" / "handmade-inherit.pptx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", root_rels),
        ("ppt/presentation.xml", presentation),
        ("ppt/_rels/presentation.xml.rels", pres_rels),
        ("ppt/slides/slide1.xml", slide),
        ("ppt/slides/_rels/slide1.xml.rels", slide_rels),
        ("ppt/slideLayouts/slideLayout1.xml", layout),
        ("ppt/slideLayouts/_rels/slideLayout1.xml.rels", layout_rels),
        ("ppt/slideMasters/slideMaster1.xml", master),
    ])


# ---------------------------------------------------------------------------
# R8: titles keep their shape-order position; p:oleObj payloads extracted

def order_pptx():
    presentation = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation {PPTX_NS}>
<p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>
</p:presentation>"""
    pres_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"""
    slide = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {PPTX_NS}>
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="Lead"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
<p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Kicker before the title</a:t></a:r></a:p></p:txBody></p:sp>
<p:sp><p:nvSpPr><p:cNvPr id="3" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
<p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Title placed second</a:t></a:r></a:p></p:txBody></p:sp>
<p:sp><p:nvSpPr><p:cNvPr id="4" name="Body"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr>
<p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Body after the title</a:t></a:r></a:p></p:txBody></p:sp>
<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="Sheet"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole">
<p:oleObj name="Quarterly numbers" progId="Excel.Sheet.12" r:id="rId2"/>
</a:graphicData></a:graphic></p:graphicFrame>
</p:spTree></p:cSld></p:sld>"""
    slide_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject" Target="../embeddings/oleObject1.bin"/>
</Relationships>"""
    root_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"""
    ct = CONTENT_TYPES_BASE.format(extra=(
        '<Default Extension="bin" ContentType="application/vnd.openxmlformats-officedocument.oleObject"/>'))
    write_zip(OUT / "pptx" / "handmade-order.pptx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", root_rels),
        ("ppt/presentation.xml", presentation),
        ("ppt/_rels/presentation.xml.rels", pres_rels),
        ("ppt/slides/slide1.xml", slide),
        ("ppt/slides/_rels/slide1.xml.rels", slide_rels),
        ("ppt/embeddings/oleObject1.bin", b"OLE-PAYLOAD-STAND-IN" * 4),
    ])


# ---------------------------------------------------------------------------
# R7: compressed .doc text decodes via the FIB language id's code page

def word_doc_stream(lid, text_bytes, far_east=False):
    """A minimal WordDocument stream: FIB base + legacy single-piece text
    (fcMin/fcMac, no Clx), with the text stored fc-compressed (one byte per
    CP) at offset 0x400."""
    fib = bytearray(0x400)
    struct.pack_into("<H", fib, 0x00, 0xA5EC)          # wIdent
    struct.pack_into("<H", fib, 0x02, 0x00C1)          # nFib (Word 97)
    flags = 0x4000 if far_east else 0                  # fFarEast
    struct.pack_into("<H", fib, 0x0A, flags)
    if far_east:
        struct.pack_into("<H", fib, 0x3C, lid)         # FibRgW97.lidFE
    else:
        struct.pack_into("<H", fib, 0x06, lid)         # FibBase.lid
    struct.pack_into("<I", fib, 0x18, 0x400)           # fcMin
    struct.pack_into("<I", fib, 0x1C, 0x400 + len(text_bytes))  # fcMac
    struct.pack_into("<I", fib, 0x4C, len(text_bytes))  # ccpText (byte CPs)
    return bytes(fib) + text_bytes


def encoded_docs():
    d = OUT / "doc"
    sj = "こんにちは世界。\r日本語の段落です。\r".encode("shift_jis")
    write_cfb(d / "handmade-shiftjis.doc",
              [("WordDocument", word_doc_stream(0x0411, sj, far_east=True))])
    ru = "Привет, мир!\rВторой абзац по-русски.\r".encode("cp1251")
    write_cfb(d / "handmade-cyrillic.doc",
              [("WordDocument", word_doc_stream(0x0419, ru))])


# ---------------------------------------------------------------------------
# R5: internal ODF gaps keep their coordinates; only trailing filler elided

def gaps_ods():
    content = """<?xml version="1.0"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
<office:body><office:spreadsheet>
<table:table table:name="RowGap">
<table:table-row><table:table-cell office:value-type="string"><text:p>top</text:p></table:table-cell></table:table-row>
<table:table-row table:number-rows-repeated="120"><table:table-cell/></table:table-row>
<table:table-row><table:table-cell office:value-type="string"><text:p>row 122 after the gap</text:p></table:table-cell></table:table-row>
<table:table-row table:number-rows-repeated="500"><table:table-cell/></table:table-row>
</table:table>
<table:table table:name="CellGap">
<table:table-row>
<table:table-cell office:value-type="string"><text:p>left</text:p></table:table-cell>
<table:table-cell table:number-columns-repeated="1010"/>
<table:table-cell office:value-type="string"><text:p>column 1012</text:p></table:table-cell>
<table:table-cell table:number-columns-repeated="200"/>
</table:table-row>
</table:table>
</office:spreadsheet></office:body></office:document-content>"""
    write_zip(OUT / "ods" / "handmade-gaps.ods",
              [("content.xml", content)],
              mimetype_first="application/vnd.oasis.opendocument.spreadsheet")


# ---------------------------------------------------------------------------
# R1: ISO 29500 Strict namespaces and rels-driven part discovery

STRICT_W = 'xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main"'
STRICT_PPTX_NS = ('xmlns:a="http://purl.oclc.org/ooxml/drawingml/main" '
                  'xmlns:p="http://purl.oclc.org/ooxml/presentationml/main" '
                  'xmlns:r="http://purl.oclc.org/ooxml/officeDocument/relationships"')


def strict_alt_ooxml():
    ct = CONTENT_TYPES_BASE.format(extra="")

    # Strict DOCX: every namespace and relationship type from the Strict
    # family; styles resolved through the document's typed relationship.
    strict_doc = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                  f'<w:document {STRICT_W}><w:body>'
                  '<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>'
                  '<w:r><w:t>Strict heading</w:t></w:r></w:p>'
                  '<w:p><w:r><w:t xml:space="preserve">Strict body with </w:t></w:r>'
                  '<w:r><w:rPr><w:rStyle w:val="Bold"/></w:rPr>'
                  '<w:t>toggled bold</w:t></w:r></w:p>'
                  '</w:body></w:document>')
    strict_styles = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                     f'<w:styles {STRICT_W}>'
                     '<w:docDefaults><w:rPrDefault><w:rPr/></w:rPrDefault></w:docDefaults>'
                     '<w:style w:type="paragraph" w:styleId="Heading1">'
                     '<w:name w:val="heading 1"/></w:style>'
                     '<w:style w:type="character" w:styleId="Bold">'
                     '<w:name w:val="Bold"/><w:rPr><w:b/></w:rPr></w:style>'
                     '</w:styles>')
    strict_root_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"""
    strict_doc_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/styles" Target="styles.xml"/>
</Relationships>"""
    write_zip(OUT / "docx" / "handmade-strict.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", strict_root_rels),
        ("word/document.xml", strict_doc),
        ("word/_rels/document.xml.rels", strict_doc_rels),
        ("word/styles.xml", strict_styles),
    ])

    # Alternate-location DOCX: no part lives at its conventional path; the
    # main part, styles, and footnotes are all found through relationships.
    alt_doc = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
               f'<w:document {W}><w:body>'
               '<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>'
               '<w:r><w:t>Relocated heading</w:t></w:r></w:p>'
               '<w:p><w:r><w:t xml:space="preserve">Body found via rels</w:t></w:r>'
               '<w:r><w:rPr><w:rStyle w:val="FootnoteRef"/></w:rPr>'
               '<w:footnoteReference w:id="2"/></w:r></w:p>'
               '</w:body></w:document>')
    alt_styles = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                  f'<w:styles {W}>'
                  '<w:docDefaults><w:rPrDefault><w:rPr/></w:rPrDefault></w:docDefaults>'
                  '<w:style w:type="paragraph" w:styleId="Heading1">'
                  '<w:name w:val="heading 1"/></w:style>'
                  '<w:style w:type="character" w:styleId="FootnoteRef">'
                  '<w:name w:val="footnote reference"/></w:style>'
                  '</w:styles>')
    alt_footnotes = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                     f'<w:footnotes {W}>'
                     '<w:footnote w:type="separator" w:id="0"><w:p/></w:footnote>'
                     '<w:footnote w:id="2"><w:p><w:r>'
                     '<w:t>A relocated footnote</w:t></w:r></w:p></w:footnote>'
                     '</w:footnotes>')
    alt_root_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="content/main.xml"/>
</Relationships>"""
    alt_doc_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="wordstyles.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes" Target="n/footnotes.xml"/>
</Relationships>"""
    write_zip(OUT / "docx" / "handmade-altpath.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", alt_root_rels),
        ("content/main.xml", alt_doc),
        ("content/_rels/main.xml.rels", alt_doc_rels),
        ("content/wordstyles.xml", alt_styles),
        ("content/n/footnotes.xml", alt_footnotes),
    ])

    # Strict PPTX: Strict namespaces plus Strict relationship types down the
    # presentation -> slide chain.
    strict_pres = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                   f'<p:presentation {STRICT_PPTX_NS}>'
                   '<p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>'
                   '</p:presentation>')
    strict_slide = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                    f'<p:sld {STRICT_PPTX_NS}>'
                    '<p:cSld><p:spTree>'
                    '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>'
                    '<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/>'
                    '<p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>'
                    '<p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Strict slide title</a:t></a:r></a:p></p:txBody></p:sp>'
                    '<p:sp><p:nvSpPr><p:cNvPr id="3" name="Body"/><p:cNvSpPr/>'
                    '<p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr>'
                    '<p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Strict body text</a:t></a:r></a:p></p:txBody></p:sp>'
                    '</p:spTree></p:cSld></p:sld>')
    strict_pres_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"""
    strict_pptx_root_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"""
    write_zip(OUT / "pptx" / "handmade-strict.pptx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", strict_pptx_root_rels),
        ("ppt/presentation.xml", strict_pres),
        ("ppt/_rels/presentation.xml.rels", strict_pres_rels),
        ("ppt/slides/slide1.xml", strict_slide),
    ])

    # Alternate-location PPTX: presentation and slide at unconventional
    # paths, reachable only through relationships.
    alt_pres = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                f'<p:presentation {PPTX_NS}>'
                '<p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>'
                '</p:presentation>')
    alt_slide = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                 f'<p:sld {PPTX_NS}>'
                 '<p:cSld><p:spTree>'
                 '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>'
                 '<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/>'
                 '<p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>'
                 '<p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Relocated deck title</a:t></a:r></a:p></p:txBody></p:sp>'
                 '</p:spTree></p:cSld></p:sld>')
    alt_pres_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="s/one.xml"/>
</Relationships>"""
    alt_pptx_root_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="deck/pres.xml"/>
</Relationships>"""
    write_zip(OUT / "pptx" / "handmade-altpath.pptx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", alt_pptx_root_rels),
        ("deck/pres.xml", alt_pres),
        ("deck/_rels/pres.xml.rels", alt_pres_rels),
        ("deck/s/one.xml", alt_slide),
    ])


# ---------------------------------------------------------------------------
# Handmade EPUB: rowspan, ol attrs, CSS display:none, non-heading anchors (M7, H13)

def features_epub():
    css = "p.hidden { display: none; }\n.crossed { text-decoration: line-through; }\n"
    ch1 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>One</title>
<link rel="stylesheet" type="text/css" href="style.css"/></head><body>
<h1>Feature Chapter</h1>
<p style="font-weight: bold">Inline-styled bold paragraph.</p>
<p class="hidden">This hidden paragraph must not appear.</p>
<p><span class="crossed">Struck via class.</span></p>
<table>
<tr><td rowspan="2">Tall</td><td>B1</td></tr>
<tr><td>B2</td></tr>
<tr><td>A3</td><td>B3</td></tr>
</table>
<ol reversed="reversed" start="3"><li>three</li><li>two</li><li>one</li></ol>
<ol type="a"><li>alpha one</li><li value="5">jumps to five</li><li>six</li></ol>
<ol start="-1"><li>minus one</li><li>zero</li><li>one</li></ol>
<ol reversed="reversed" start="1"><li>one</li><li>zero</li><li>minus one</li></ol>
<p>See <a href="ch2.xhtml#target-span">the marked span</a> in chapter two,
or <a href="https://example.com/x">an external page</a>,
or <a href="notes.txt">a relative resource</a>.</p>
</body></html>"""
    ch2 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Two</title></head><body>
<h1>Second Chapter</h1>
<p>Leading text. <span id="target-span">This span is the anchor target.</span></p>
<p>Back to <a href="ch1.xhtml">chapter one's start</a>.</p>
</body></html>"""
    opf = """<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="uid">urn:uuid:00000000-0000-0000-0000-00000000f1x7</dc:identifier>
<dc:title>Feature Book</dc:title><dc:language>en</dc:language>
</metadata>
<manifest>
<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
<item id="c2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
<item id="css" href="style.css" media-type="text/css"/>
</manifest>
<spine><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"""
    container = """<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"""
    write_zip(OUT / "epub" / "handmade-features.epub", [
        ("META-INF/container.xml", container),
        ("OEBPS/content.opf", opf),
        ("OEBPS/ch1.xhtml", ch1),
        ("OEBPS/ch2.xhtml", ch2),
        ("OEBPS/style.css", css),
    ], mimetype_first="application/epub+zip")


# ---------------------------------------------------------------------------
# R17a: merged ranges in spreadsheets become spanning grid cells

def merged_xlsx():
    ct = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"""
    root_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"""
    workbook = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="Merged" sheetId="1" r:id="rId1"/></sheets></workbook>"""
    wb_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"""
    sheet = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>
<row r="1">
<c r="A1" t="inlineStr"><is><t>Merged across</t></is></c>
<c r="C1" t="inlineStr"><is><t xml:space="preserve">  padded  </t></is></c>
</row>
<row r="2">
<c r="A2" t="inlineStr"><is><t>tall</t></is></c>
<c r="B2" t="inlineStr"><is><t>b2</t></is></c>
<c r="C2"><v>3.5</v></c>
</row>
<row r="3"><c r="B3" t="inlineStr"><is><t>b3</t></is></c></row>
</sheetData>
<mergeCells count="2"><mergeCell ref="A1:B1"/><mergeCell ref="A2:A3"/></mergeCells>
</worksheet>"""
    write_zip(OUT / "xlsx" / "handmade-merged.xlsx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", root_rels),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", wb_rels),
        ("xl/worksheets/sheet1.xml", sheet),
    ])


# ---------------------------------------------------------------------------
# R-hidden-sheet: hidden / veryHidden worksheets must not render as visible
# content (sheet-level visibility, the half of #9 calamine exposes). A visible
# sheet keeps its table; a hidden and a veryHidden sheet drop out entirely.

def hidden_xlsx():
    ct = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
<Override PartName="/xl/worksheets/sheet3.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"""
    root_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"""
    workbook = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets>
<sheet name="Visible" sheetId="1" r:id="rId1"/>
<sheet name="Hidden" sheetId="2" r:id="rId2" state="hidden"/>
<sheet name="VeryHidden" sheetId="3" r:id="rId3" state="veryHidden"/>
</sheets></workbook>"""
    wb_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/>
</Relationships>"""
    sheet1 = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData><row r="1">
<c r="A1" t="inlineStr"><is><t>visible cell</t></is></c>
<c r="B1" t="inlineStr"><is><t>shown</t></is></c>
</row></sheetData>
</worksheet>"""
    sheet2 = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData><row r="1">
<c r="A1" t="inlineStr"><is><t>hidden cell</t></is></c>
<c r="B1" t="inlineStr"><is><t>must not appear</t></is></c>
</row></sheetData>
</worksheet>"""
    sheet3 = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData><row r="1">
<c r="A1" t="inlineStr"><is><t>very hidden cell</t></is></c>
<c r="B1" t="inlineStr"><is><t>must not appear either</t></is></c>
</row></sheetData>
</worksheet>"""
    write_zip(OUT / "xlsx" / "handmade-hidden.xlsx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", root_rels),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", wb_rels),
        ("xl/worksheets/sheet1.xml", sheet1),
        ("xl/worksheets/sheet2.xml", sheet2),
        ("xl/worksheets/sheet3.xml", sheet3),
    ])


# ---------------------------------------------------------------------------
# R16: ODF style:default-style beneath named chains; full ISO durations

def defaults_odf():
    styles_xml = """<?xml version="1.0"?>
<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
 xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0">
<office:styles>
<style:default-style style:family="paragraph">
<style:text-properties fo:font-weight="bold"/>
</style:default-style>
<style:style style:name="Norm" style:family="paragraph"/>
<style:style style:name="Off" style:family="paragraph">
<style:text-properties fo:font-weight="normal"/>
</style:style>
</office:styles>
</office:document-styles>"""
    content_xml = """<?xml version="1.0"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
<office:body><office:text>
<text:p text:style-name="Norm">Bold via the family default style</text:p>
<text:p text:style-name="Off">Explicitly back to normal weight</text:p>
<text:p>Unstyled paragraph also inherits the default</text:p>
</office:text></office:body></office:document-content>"""
    write_zip(OUT / "odt" / "handmade-defaults.odt", [
        ("content.xml", content_xml),
        ("styles.xml", styles_xml),
    ], mimetype_first="application/vnd.oasis.opendocument.text")

    durations = """<?xml version="1.0"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
<office:body><office:spreadsheet><table:table table:name="Durations">
<table:table-row>
<table:table-cell office:value-type="time" office:time-value="P1DT2H"/>
<table:table-cell office:value-type="time" office:time-value="PT26H30M15S"/>
<table:table-cell office:value-type="time" office:time-value="P2W"/>
<table:table-cell office:value-type="time" office:time-value="-PT1H5M"/>
<table:table-cell office:value-type="time" office:time-value="P1M"/>
</table:table-row>
<table:table-row>
<table:table-cell office:value-type="time" office:time-value="P0.5D"/>
<table:table-cell office:value-type="time" office:time-value="PT1.5H"/>
<table:table-cell office:value-type="time" office:time-value="PT1.5M"/>
<table:table-cell office:value-type="time" office:time-value="PT1.25S"/>
<table:table-cell office:value-type="time" office:time-value="PT99999999999999999999H"/>
</table:table-row>
</table:table></office:spreadsheet></office:body></office:document-content>"""
    write_zip(OUT / "ods" / "handmade-durations.ods",
              [("content.xml", durations)],
              mimetype_first="application/vnd.oasis.opendocument.spreadsheet")

    # S7: list headers render without markers, start-value restarts split
    # the run, and continue-list resolves by the target's xml:id.
    lists_xml = """<?xml version="1.0"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
 xmlns:xml="http://www.w3.org/XML/1998/namespace">
<office:automatic-styles>
<text:list-style style:name="Num">
<text:list-level-style-number text:level="1" style:num-format="1"/>
</text:list-style>
</office:automatic-styles>
<office:body><office:text>
<text:list xml:id="lstA" text:style-name="Num">
<text:list-header><text:p>Header without a marker</text:p></text:list-header>
<text:list-item><text:p>one</text:p></text:list-item>
<text:list-item><text:p>two</text:p></text:list-item>
<text:list-item text:start-value="10"><text:p>ten via restart</text:p></text:list-item>
<text:list-item><text:p>eleven</text:p></text:list-item>
</text:list>
<text:p>Interruption paragraph.</text:p>
<text:list text:continue-list="lstA" text:style-name="Num">
<text:list-item><text:p>twelve continues lstA</text:p></text:list-item>
</text:list>
</office:text></office:body></office:document-content>"""
    write_zip(OUT / "odt" / "handmade-lists.odt",
              [("content.xml", lists_xml)],
              mimetype_first="application/vnd.oasis.opendocument.text")

    # S18: encryption is detected from the actual manifest:encryption-data
    # element - a comment mentioning it must not classify the document as
    # encrypted, while a real element must.
    plain_content = """<?xml version="1.0"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
<office:body><office:text><text:p>Not encrypted at all.</text:p></office:text></office:body>
</office:document-content>"""
    manifest_comment = """<?xml version="1.0"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0">
<!-- this comment mentions encryption-data but the package is plain -->
<manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.text"/>
<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
</manifest:manifest>"""
    write_zip(OUT / "odt" / "handmade-manifestcomment.odt", [
        ("content.xml", plain_content),
        ("META-INF/manifest.xml", manifest_comment),
    ], mimetype_first="application/vnd.oasis.opendocument.text")
    manifest_encrypted = """<?xml version="1.0"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0">
<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml">
<manifest:encryption-data manifest:checksum-type="SHA1/1K" manifest:checksum="AAAA">
<manifest:algorithm manifest:algorithm-name="Blowfish CFB" manifest:initialisation-vector="BBBB"/>
<manifest:key-derivation manifest:key-derivation-name="PBKDF2" manifest:iteration-count="1024" manifest:salt="CCCC"/>
</manifest:encryption-data>
</manifest:file-entry>
</manifest:manifest>"""
    write_zip(OUT / "malformed" / "encrypted--errors.odt", [
        ("content.xml", b"\x00\x01ciphertext-stand-in"),
        ("META-INF/manifest.xml", manifest_encrypted),
    ], mimetype_first="application/vnd.oasis.opendocument.text")


# ---------------------------------------------------------------------------
# R15: DOCX gridBefore/gridAfter, legacy hMerge, ST_OnOff tblHeader

def tables_docx():
    def tc(text, extra=""):
        return (f'<w:tc><w:tcPr>{extra}</w:tcPr>'
                f'<w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p></w:tc>')

    document = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {W}><w:body>
<w:tbl>
<w:tr><w:trPr><w:tblHeader/></w:trPr>
{tc("Head A")}{tc("Head B")}{tc("Head C", '<w:vMerge w:val="restart"/>')}</w:tr>
<w:tr><w:trPr><w:tblHeader w:val="0"/><w:gridBefore w:val="2"/></w:trPr>
{tc("under C", '<w:vMerge/>')}</w:tr>
<w:tr><w:trPr><w:gridAfter w:val="1"/></w:trPr>
{tc("legacy start", '<w:hMerge w:val="restart"/>')}{tc("legacy cont", '<w:hMerge/>')}</w:tr>
</w:tbl>
<w:p><w:r><w:t>after the table</w:t></w:r></w:p>
</w:body></w:document>"""
    styles = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
              f'<w:styles {W}>'
              '<w:docDefaults><w:rPrDefault><w:rPr/></w:rPrDefault></w:docDefaults></w:styles>')
    ct = CONTENT_TYPES_BASE.format(extra="")
    write_zip(OUT / "docx" / "handmade-tables.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", ROOT_RELS),
        ("word/document.xml", document),
        ("word/styles.xml", styles),
    ])


# ---------------------------------------------------------------------------
# S12: a valid document referencing one part many times must convert with a
# single retained asset; the part is cached and the asset deduplicated. (The
# budget re-charging itself is covered by unit tests - a fixture big enough
# to cross the 512 MiB budget would slow the mutation harness down.)

def manyrefs_docx():
    drawing = ('<w:p><w:r><w:drawing>'
               '<a:blip xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
               ' r:embed="rId10"/>'
               '</w:drawing></w:r></w:p>')
    document = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                f'<w:document {W} {R}><w:body>'
                '<w:p><w:r><w:t>Seventy references to one image follow.</w:t></w:r></w:p>'
                + drawing * 70 +
                '</w:body></w:document>')
    doc_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId10" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/logo.png"/>
</Relationships>"""
    ct = CONTENT_TYPES_BASE.format(extra='<Default Extension="png" ContentType="image/png"/>')
    write_zip(OUT / "docx" / "handmade-manyrefs.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", ROOT_RELS),
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", doc_rels),
        ("word/media/logo.png", b"\x89PNG\r\n\x1a\n" + b"\x00" * (64 * 1024)),
    ])


# ---------------------------------------------------------------------------
# S9: links to another slide resolve to the target slide's start anchor
# instead of a dead relative reference to slides/slideN.xml.

def links_pptx():
    presentation = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation {PPTX_NS}>
<p:sldIdLst><p:sldId id="256" r:id="rId1"/><p:sldId id="257" r:id="rId2"/></p:sldIdLst>
</p:presentation>"""
    pres_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/>
</Relationships>"""

    def slide(body):
        return f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {PPTX_NS}>
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
{body}
</p:spTree></p:cSld></p:sld>"""

    slide1 = slide(
        '<p:sp><p:nvSpPr><p:cNvPr id="2" name="Body"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>'
        '<p:spPr/><p:txBody><a:bodyPr/>'
        '<a:p><a:r><a:rPr><a:hlinkClick r:id="rId5"/></a:rPr><a:t>Jump to the second slide</a:t></a:r></a:p>'
        '<a:p><a:r><a:rPr><a:hlinkClick r:id="rId6"/></a:rPr><a:t>External link</a:t></a:r></a:p>'
        '</p:txBody></p:sp>')
    slide1_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slide2.xml"/>
<Relationship Id="rId6" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/>
</Relationships>"""
    slide2 = slide(
        '<p:sp><p:nvSpPr><p:cNvPr id="2" name="Body"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>'
        '<p:spPr/><p:txBody><a:bodyPr/>'
        '<a:p><a:r><a:t>Second slide content</a:t></a:r></a:p>'
        '</p:txBody></p:sp>')
    ct = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
<Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
<Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>"""
    root_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"""
    write_zip(OUT / "pptx" / "handmade-links.pptx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", root_rels),
        ("ppt/presentation.xml", presentation),
        ("ppt/_rels/presentation.xml.rels", pres_rels),
        ("ppt/slides/slide1.xml", slide1),
        ("ppt/slides/_rels/slide1.xml.rels", slide1_rels),
        ("ppt/slides/slide2.xml", slide2),
    ])


# ---------------------------------------------------------------------------
# S5: standard Word OLE markup - a VML preview image next to the
# o:OLEObject; the object's identity and payload must win over the preview.

def ole_docx():
    V = 'xmlns:v="urn:schemas-microsoft-com:vml"'
    O = 'xmlns:o="urn:schemas-microsoft-com:office:office"'
    document = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {W} {R} {V} {O}><w:body>
<w:p><w:r><w:t>Before the object</w:t></w:r></w:p>
<w:p><w:r><w:object>
<v:shape id="_x0000_i1025" style="width:96pt;height:48pt">
<v:imagedata r:id="rId3" o:title="preview"/>
</v:shape>
<o:OLEObject Type="Embed" ProgID="Excel.Sheet.12" ShapeID="_x0000_i1025" r:id="rId2"/>
</w:object></w:r></w:p>
</w:body></w:document>"""
    doc_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject" Target="embeddings/oleObject1.bin"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/preview.png"/>
</Relationships>"""
    styles = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
              f'<w:styles {W}>'
              '<w:docDefaults><w:rPrDefault><w:rPr/></w:rPrDefault></w:docDefaults></w:styles>')
    ct = CONTENT_TYPES_BASE.format(extra=(
        '<Default Extension="bin" ContentType="application/vnd.openxmlformats-officedocument.oleObject"/>'
        '<Default Extension="png" ContentType="image/png"/>'))
    write_zip(OUT / "docx" / "handmade-ole.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", ROOT_RELS),
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", doc_rels),
        ("word/styles.xml", styles),
        ("word/embeddings/oleObject1.bin", b"DOCX-OLE-PAYLOAD" * 4),
        ("word/media/preview.png", b"\x89PNG\r\n\x1a\nfakepreview"),
    ])


# ---------------------------------------------------------------------------
# S2: outline level is a tri-state property; direct paragraph properties
# override the style chain, and outlineLvl 9 is the explicit off state.

def outline_docx():
    def p(text, ppr=""):
        pr = f"<w:pPr>{ppr}</w:pPr>" if ppr else ""
        return f'<w:p>{pr}<w:r><w:t>{text}</w:t></w:r></w:p>'

    document = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {W}><w:body>
{p("Style heading stays a heading", '<w:pStyle w:val="Outlined"/>')}
{p("Direct level overrides the style", '<w:pStyle w:val="Outlined"/><w:outlineLvl w:val="2"/>')}
{p("Direct nine turns the style heading off", '<w:pStyle w:val="Outlined"/><w:outlineLvl w:val="9"/>')}
{p("Direct outline without a style", '<w:outlineLvl w:val="0"/>')}
{p("Child style nine stops inheritance", '<w:pStyle w:val="NotOutlined"/>')}
</w:body></w:document>"""
    styles = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles {W}>
<w:docDefaults><w:rPrDefault><w:rPr/></w:rPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:styleId="Outlined"><w:name w:val="Outlined"/>
<w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="NotOutlined"><w:name w:val="NotOutlined"/>
<w:basedOn w:val="Outlined"/><w:pPr><w:outlineLvl w:val="9"/></w:pPr></w:style>
</w:styles>"""
    ct = CONTENT_TYPES_BASE.format(extra="")
    write_zip(OUT / "docx" / "handmade-outline.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", ROOT_RELS),
        ("word/document.xml", document),
        ("word/styles.xml", styles),
    ])


# ---------------------------------------------------------------------------
# R14: binary PPT per-slide master selection with tri-state level defaults

def ppt_rec(ver_inst, rec_type, body):
    return struct.pack("<HHI", ver_inst, rec_type, len(body)) + body


def ppt_container(instance, rec_type, body):
    return ppt_rec((instance << 4) | 0xF, rec_type, body)


def multimaster_ppt():
    # TxMasterStyleAtom instance 1 (body text), one level: PF carries
    # hasBullet, CF carries exactly one specified style bit (tri-state).
    def tx_master(bullet_on, cf_mask, cf_style):
        pf = struct.pack("<IH", 0x0001, 0x0001 if bullet_on else 0x0000)
        cf = struct.pack("<IH", cf_mask, cf_style)
        return ppt_rec(1 << 4, 0x0FA3, struct.pack("<H", 1) + pf + cf)

    def slide(master_id, text):
        slide_atom = ppt_rec(2, 0x03EF, struct.pack("<I", 0) + b"\x00" * 8
                             + struct.pack("<IIHH", master_id, 0, 0, 0))
        th = ppt_rec(0, 0x0F9F, struct.pack("<I", 1))
        tb = ppt_rec(0, 0x0FA8, text.encode("ascii"))
        return ppt_container(0, 0x03EE, slide_atom + th + tb)

    def persist_atom(persist_ref, sid):
        return ppt_rec(0, 0x03F3, struct.pack("<IIIII", persist_ref, 0, 0, sid, 0))

    slide_list = ppt_container(0, 0x0FF0, persist_atom(4, 256) + persist_atom(5, 257))
    master_list = ppt_container(1, 0x0FF0, persist_atom(2, 1001) + persist_atom(3, 1002))
    doc = ppt_container(0, 0x03E8, slide_list + master_list)
    master_a = ppt_container(0, 0x03F8, tx_master(True, 0x0001, 0x0001))   # bullet+bold
    master_b = ppt_container(0, 0x03F8, tx_master(False, 0x0002, 0x0002))  # no bullet, italic
    slide1 = slide(1001, "Alpha master body text\r")
    slide2 = slide(1002, "Beta master body text\r")

    stream = b""
    offsets = {}
    for pid, blob in [(1, doc), (2, master_a), (3, master_b), (4, slide1), (5, slide2)]:
        offsets[pid] = len(stream)
        stream += blob
    entries = struct.pack("<I", 1 | (5 << 20)) + struct.pack(
        "<5I", *(offsets[i] for i in range(1, 6)))
    off_dir = len(stream)
    stream += ppt_rec(0, 0x1772, entries)
    off_edit = len(stream)
    stream += ppt_rec(0, 0x0FF5, struct.pack(
        "<IIIIIIHH", 0, 0, 0, off_dir, 1, 6, 0, 0))
    current_user = ppt_rec(0, 0x0FF6, struct.pack("<III", 20, 0xE391C05F, off_edit))
    write_cfb(OUT / "ppt" / "handmade-multimaster.ppt", [
        ("Current User", current_user),
        ("PowerPoint Document", stream),
    ])


# ---------------------------------------------------------------------------
# S20: notes pages pair to slides via NotesAtom.slideIdRef, not list order -
# this deck has notes only on the SECOND slide, which order-based zipping
# would misattribute to the first.

def sparsenotes_ppt():
    def slide(text):
        slide_atom = ppt_rec(2, 0x03EF, struct.pack("<I", 0) + b"\x00" * 8
                             + struct.pack("<IIHH", 0, 0, 0, 0))
        th = ppt_rec(0, 0x0F9F, struct.pack("<I", 1))
        tb = ppt_rec(0, 0x0FA8, text.encode("ascii"))
        return ppt_container(0, 0x03EE, slide_atom + th + tb)

    def notes(slide_ref, text):
        notes_atom = ppt_rec(1, 0x03F1, struct.pack("<IHH", slide_ref, 0, 0))
        th = ppt_rec(0, 0x0F9F, struct.pack("<I", 2))
        tb = ppt_rec(0, 0x0FA8, text.encode("ascii"))
        return ppt_container(0, 0x03F0, notes_atom + th + tb)

    def persist_atom(persist_ref, sid):
        return ppt_rec(0, 0x03F3, struct.pack("<IIIII", persist_ref, 0, 0, sid, 0))

    slide_list = ppt_container(0, 0x0FF0, persist_atom(2, 256) + persist_atom(3, 257))
    notes_list = ppt_container(2, 0x0FF0, persist_atom(4, 1301))
    doc = ppt_container(0, 0x03E8, slide_list + notes_list)
    s1 = slide("First slide text\r")
    s2 = slide("Second slide text\r")
    n2 = notes(257, "Notes for the second slide\r")

    stream = b""
    offsets = {}
    for pid, blob in [(1, doc), (2, s1), (3, s2), (4, n2)]:
        offsets[pid] = len(stream)
        stream += blob
    entries = struct.pack("<I", 1 | (4 << 20)) + struct.pack(
        "<4I", *(offsets[i] for i in range(1, 5)))
    off_dir = len(stream)
    stream += ppt_rec(0, 0x1772, entries)
    off_edit = len(stream)
    stream += ppt_rec(0, 0x0FF5, struct.pack(
        "<IIIIIIHH", 0, 0, 0, off_dir, 1, 5, 0, 0))
    current_user = ppt_rec(0, 0x0FF6, struct.pack("<III", 20, 0xE391C05F, off_edit))
    write_cfb(OUT / "ppt" / "handmade-sparsenotes.ppt", [
        ("Current User", current_user),
        ("PowerPoint Document", stream),
    ])


# ---------------------------------------------------------------------------
# R13: \sbasedon style chains; boundary-matched merges in nonuniform tables

def merge_rtf():
    parts = [
        rb"{\rtf1\ansi\ansicpg1252\deff0",
        rb"{\fonttbl{\f0\fcharset0 Arial;}}",
        rb"{\stylesheet{\s1\b\outlinelevel1 Base Bold;}"
        rb"{\s2\sbasedon1\i Child Italic;}}",
        # The paragraph carries only \s2: bold and the outline level must
        # arrive through the \sbasedon chain.
        rb"\pard\s2 Inherited heading\par",
        rb"\pard\plain Plain paragraph between tables.\par",
        # Nonuniform boundaries: row 2's wide cell spans what rows 1 and 3
        # divide differently; the \clvmrg chain matches by \cellx boundary.
        rb"\trowd\clvmgf\cellx3000\cellx6000\cellx9000 A1\cell B1\cell C1\cell\row",
        rb"\trowd\clvmrg\cellx3000\cellx9000 \cell Wide B2\cell\row",
        rb"\trowd\cellx4500\cellx9000 A3 wide\cell B3 wide\cell\row",
        # A stray \clvmrg with no matching chain above stays visible.
        rb"\trowd\clvmrg\cellx4500\cellx9000 stray continuation\cell X\cell\row",
        rb"}",
    ]
    (OUT / "rtf").mkdir(parents=True, exist_ok=True)
    (OUT / "rtf" / "handmade-merge.rtf").write_bytes(b"\n".join(parts))


# ---------------------------------------------------------------------------
# R10/R11: per-chapter CSS cascades and spine-only anchor classification

def css_links_epub():
    css_a = ".mark { font-weight: bold; }\n"
    css_b = ".mark { font-style: italic; font-weight: normal; }\n"
    ch1 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>One</title>
<link rel="stylesheet" type="text/css" href="style-a.css"/></head><body>
<h1>Sheet A Chapter</h1>
<p><span class="mark">Bold via sheet A.</span></p>
<p>See <a href="pic.png">the picture</a>,
<a href="data.bin">the download</a>,
<a href="appendix.xhtml">the non-linear appendix</a>,
or <a href="ch2.xhtml">chapter two</a>.</p>
</body></html>"""
    ch2 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Two</title>
<link rel="stylesheet" type="text/css" href="style-b.css"/>
<style>p.local { text-decoration: line-through; }</style></head><body>
<h1>Sheet B Chapter</h1>
<p><span class="mark">Italic via sheet B only.</span></p>
<p class="local">Struck via the chapter style block.</p>
</body></html>"""
    ch3 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Three</title>
<link rel="stylesheet" type="text/css" href="style-a.css"/>
<link rel="stylesheet" type="text/css" href="style-b.css"/></head><body>
<h1>Cascade Chapter</h1>
<p><span class="mark">Sheet B wins the conflict.</span></p>
</body></html>"""
    ch4 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Four</title>
<style>
span.secret { display: none; }
.secret { display: block; }
span { display: none; }
.visible { display: block; }
p.strong { font-weight: bold; }
.strong { font-weight: normal; }
.gone { display: none; }
</style></head><body>
<h1>Specificity Chapter</h1>
<p><span class="secret">Hidden: tag.class outweighs the restoring class.</span>after the span</p>
<p><span class="visible">Restored: the class rule outweighs the hiding tag rule.</span></p>
<p class="strong">Bold: tag.class beats the later class rule.</p>
<p class="gone">Never shown.</p>
</body></html>"""
    appendix = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Appendix</title></head><body>
<p>Non-linear content, not converted.</p>
</body></html>"""
    opf = """<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="uid">urn:uuid:00000000-0000-0000-0000-00000000c55l</dc:identifier>
<dc:title>Cascade Book</dc:title><dc:language>en</dc:language>
</metadata>
<manifest>
<item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
<item id="c2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
<item id="c3" href="ch3.xhtml" media-type="application/xhtml+xml"/>
<item id="c4" href="ch4.xhtml" media-type="application/xhtml+xml"/>
<item id="app" href="appendix.xhtml" media-type="application/xhtml+xml"/>
<item id="cssa" href="style-a.css" media-type="text/css"/>
<item id="cssb" href="style-b.css" media-type="text/css"/>
<item id="pic" href="pic.png" media-type="image/png"/>
<item id="bin" href="data.bin" media-type="application/octet-stream"/>
</manifest>
<spine><itemref idref="c1"/><itemref idref="c2"/><itemref idref="c3"/><itemref idref="c4"/>
<itemref idref="app" linear="no"/></spine>
</package>"""
    container = """<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"""
    write_zip(OUT / "epub" / "handmade-css-links.epub", [
        ("META-INF/container.xml", container),
        ("OEBPS/content.opf", opf),
        ("OEBPS/ch1.xhtml", ch1),
        ("OEBPS/ch2.xhtml", ch2),
        ("OEBPS/ch3.xhtml", ch3),
        ("OEBPS/ch4.xhtml", ch4),
        ("OEBPS/appendix.xhtml", appendix),
        ("OEBPS/style-a.css", css_a),
        ("OEBPS/style-b.css", css_b),
        ("OEBPS/pic.png", DOT_PNG),
        ("OEBPS/data.bin", b"NOT-A-DOCUMENT"),
    ], mimetype_first="application/epub+zip")


# ---------------------------------------------------------------------------
# Handmade RTF: \bin payload, list table, nested table, per-font charset (H11, H12, M13)

def bin_rtf():
    payload = b"AB{C}D\\E}}{F.."  # 14 raw bytes full of grouping chars
    assert len(payload) == 14
    parts = [
        rb"{\rtf1\ansi\ansicpg1252\deff0",
        rb"{\fonttbl{\f0\fcharset0 Arial;}{\f1\fcharset204 Arial;}}",
        rb"{\stylesheet{\s1\outlinelevel0\b Heading One;}{\s0 Normal;}}",
        rb"{\*\listtable{\list\listtemplateid1{\listlevel\levelnfc2\levelstartat4"
        rb"{\leveltext \'02\'00.;}{\levelnumbers \'01;}}\listid100}}",
        rb"{\*\listoverridetable{\listoverride\listid100\listoverridecount0\ls1}"
        # S6: per-level override records - an overridden start, and an
        # overriding format via an embedded \listlevel
        rb"{\listoverride\listid100\listoverridecount1"
        rb"{\lfolevel\listoverridestartat\levelstartat10}\ls2}"
        rb"{\listoverride\listid100\listoverridecount1"
        rb"{\lfolevel\listoverrideformat{\listlevel\levelnfc0\levelstartat7}}\ls3}}",
        rb"\pard\s1\b Heading via style\b0\par",
        rb"\pard Before the binary blob.\par",
        rb"{\*\pict\wmetafile8\bin14 " + payload + rb"}",
        rb"\pard After the binary blob: braces {stay} balanced.\par",
        rb"\pard{\listtext IV.\tab}\ls1\ilvl0 Roman four via list table\par",
        rb"\pard{\listtext V.\tab}\ls1\ilvl0 Roman five\par",
        rb"\pard{\listtext X.\tab}\ls2\ilvl0 Roman ten via start override\par",
        rb"\pard{\listtext XI.\tab}\ls2\ilvl0 Roman eleven\par",
        rb"\pard{\listtext 7.\tab}\ls3\ilvl0 Decimal seven via format override\par",
        # simple table with a merged first row and a header row
        rb"\trowd\trhdr\clmgf\cellx4000\clmrg\cellx8000 Wide header\cell\cell\row",
        rb"\trowd\cellx4000\cellx8000 L\cell R\cell\row",
        # nested table inside the first cell of an outer table
        rb"\trowd\cellx4000\cellx8000",
        rb"\pard\intbl\itap2 nested A\nestcell nested B\nestcell"
        rb"{\*\nesttableprops\trowd\cellx2000\cellx4000\nestrow}",
        rb"\pard\intbl outer second\cell\row",
        # cp1251 text under \f1 (fcharset204): bytes for Russian "Da"
        rb"\pard\f1 \'c4\'e0\f0\par",
        rb"\pard Persian join: \u1605?\u1740?\u8204?\u1582?\par",
        rb"}",
    ]
    data = b"\n".join(parts)
    (OUT / "rtf").mkdir(parents=True, exist_ok=True)
    (OUT / "rtf" / "handmade-bin.rtf").write_bytes(data)
    # unbalanced variant: drop the final closing brace -> recovers
    (OUT / "malformed").mkdir(parents=True, exist_ok=True)
    (OUT / "malformed" / "unbalanced--recovers.rtf").write_bytes(data[: data.rfind(b"}")])


# ---------------------------------------------------------------------------
# Handmade CSVs (M8)

def csvs():
    d = OUT / "csv"
    d.mkdir(parents=True, exist_ok=True)
    (d / "handmade-quoted.csv").write_text(
        'name,desc,qty\n"  padded  ","comma, inside",3\nplain,"multi\nline",4\n',
        encoding="utf-8", newline="")
    (d / "handmade-semicolon.csv").write_text(
        'a;b;c\n"1,5";"2,5";x\n"3,0";y;z\n', encoding="utf-8", newline="")
    (d / "handmade-utf16.csv").write_bytes(
        b"\xff\xfe" + "col1,col2\nnaïve,café\nΑθήνα,数据\n".encode("utf-16-le"))


# ---------------------------------------------------------------------------
# Malformed set (recover / skip / ignore / error annotations in filenames)

def zip_without(src, dst, drop):
    with zipfile.ZipFile(src) as zin:
        entries = [(i.filename, zin.read(i.filename))
                   for i in zin.infolist() if i.filename != drop]
    write_zip(dst, entries)


def zip_replace(src, dst, name, data):
    with zipfile.ZipFile(src) as zin:
        entries = [(i.filename, data if i.filename == name else zin.read(i.filename))
                   for i in zin.infolist()]
    write_zip(dst, entries)


def malformed():
    m = OUT / "malformed"
    m.mkdir(parents=True, exist_ok=True)
    docx = OUT / "docx" / "text.docx"
    if docx.exists():
        raw = docx.read_bytes()
        (m / "truncated--errors.docx").write_bytes(raw[: len(raw) * 3 // 5])
        zip_without(docx, m / "missing-styles--skips.docx", "word/styles.xml")
        zip_replace(docx, m / "corrupt-styles--skips.docx", "word/styles.xml",
                    b"\x00\x01garbage <notxml")
    # hand-built recovery-class XML shapes
    unclosed_doc = (f'<?xml version="1.0"?><w:document {W}><w:body>'
                    f'<w:p><w:r><w:t>Unclosed paragraph text')
    mismatched_doc = (f'<?xml version="1.0"?><w:document {W}><w:body>'
                      f'<w:p><w:r><w:t>Mismatched tags</w:t></w:r></w:q>'
                      f'</w:body></w:document>')
    styles_min = (f'<?xml version="1.0"?><w:styles {W}/>')
    ct = CONTENT_TYPES_BASE.format(extra="")
    for name, doc in [("unclosed--recovers.docx", unclosed_doc),
                      ("mismatched--recovers.docx", mismatched_doc)]:
        write_zip(m / name, [
            ("[Content_Types].xml", ct),
            ("_rels/.rels", ROOT_RELS),
            ("word/document.xml", doc),
            ("word/styles.xml", styles_min),
        ])
    (m / "empty--errors.docx").write_bytes(b"")
    doc = OUT / "doc" / "text.doc"
    if doc.exists():
        (m / "truncated--errors.doc").write_bytes(doc.read_bytes()[:4096])
    ppt = OUT / "ppt" / "pres.ppt"
    if ppt.exists():
        # Corrupt every UserEditAtom record type so the persist directory is
        # structurally unusable -> the labelled raw-recovery path must run.
        raw = ppt.read_bytes().replace(b"\xf5\x0f", b"\xf5\xff")
        (m / "brokenpersist--recovers.ppt").write_bytes(raw)


def abuse():
    a = OUT / "abuse"
    a.mkdir(parents=True, exist_ok=True)
    ct = CONTENT_TYPES_BASE.format(extra="")
    # The bomb must live in a part the parser actually reads: a document.xml
    # that decompresses to ~192 MiB crosses the 128 MiB per-entry cap.
    bomb_doc = (f'<?xml version="1.0"?><w:document {W}><w:body><w:p><w:r><w:t>'
                + "A" * (192 * 1024 * 1024)
                + "</w:t></w:r></w:p></w:body></w:document>")
    write_zip(a / "zipbomb--errors.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", ROOT_RELS),
        ("word/document.xml", bomb_doc),
    ])
    # R2: the bomb lives in an *optional* part (an image behind a
    # relationship). Resource limits must propagate out of recovery paths
    # instead of being skipped like an ordinary broken image.
    image_doc = (f'<?xml version="1.0"?><w:document {W} {R}><w:body>'
                 '<w:p><w:r><w:t>before the image</w:t></w:r></w:p>'
                 '<w:p><w:r><w:drawing>'
                 '<a:blip xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
                 ' r:embed="rId10"/>'
                 '</w:drawing></w:r></w:p>'
                 '</w:body></w:document>')
    image_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId10" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"""
    write_zip(a / "imagebomb--errors.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", ROOT_RELS),
        ("word/document.xml", image_doc),
        ("word/_rels/document.xml.rels", image_rels),
        ("word/media/image1.png", b"\x00" * (192 * 1024 * 1024)),
    ])
    depth = 40_000
    deep_doc = (f'<?xml version="1.0"?><w:document {W}><w:body><w:p><w:r><w:t>'
                + "<x>" * depth + "deep" + "</x>" * depth
                + "</w:t></w:r></w:p></w:body></w:document>")
    write_zip(a / "deepxml--errors.docx", [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", ROOT_RELS),
        ("word/document.xml", deep_doc),
    ])
    # R6: pathologically nested binary PPT containers; the bounded record
    # walk must fail with ResourceLimit instead of exhausting the stack.
    nested = struct.pack("<HHI", 0x0000, 0x0FA8, 4) + b"deep"
    for _ in range(200):
        nested = struct.pack("<HHI", 0x000F, 0x03EE, len(nested)) + nested
    write_cfb(a / "deepnest--errors.ppt", [
        ("Current User", b""),
        ("PowerPoint Document", nested),
    ])
    repeat_ods = """<?xml version="1.0"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
<office:body><office:spreadsheet><table:table table:name="S">
<table:table-row table:number-rows-repeated="9000000">
<table:table-cell office:value-type="string" table:number-columns-repeated="1000"><text:p>x</text:p></table:table-cell>
</table:table-row>
</table:table></office:spreadsheet></office:body></office:document-content>"""
    write_zip(a / "hugerepeat--errors.ods",
              [("content.xml", repeat_ods)],
              mimetype_first="application/vnd.oasis.opendocument.spreadsheet")
    # S1: a single cell whose declared span area is astronomically large; the
    # complete area must be charged against the expansion budget *before* any
    # per-position work happens.
    span_ods = """<?xml version="1.0"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
<office:body><office:spreadsheet><table:table table:name="S">
<table:table-row>
<table:table-cell table:number-rows-spanned="2000000000" table:number-columns-spanned="2000000000"><text:p>x</text:p></table:table-cell>
</table:table-row>
</table:table></office:spreadsheet></office:body></office:document-content>"""
    write_zip(a / "hugespan--errors.ods",
              [("content.xml", span_ods)],
              mimetype_first="application/vnd.oasis.opendocument.spreadsheet")
    # S1: a repeated row whose cells are structurally present but empty used
    # to bypass the budget entirely (nothing content-bearing was ever
    # charged) while still looping over the repeat count.
    emptyrepeat_ods = """<?xml version="1.0"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
<office:body><office:spreadsheet><table:table table:name="S">
<table:table-row table:number-rows-repeated="9000000000000000"><table:table-cell><text:p/></table:table-cell></table:table-row>
<table:table-row><table:table-cell><text:p>tail</text:p></table:table-cell></table:table-row>
</table:table></office:spreadsheet></office:body></office:document-content>"""
    write_zip(a / "emptyrowrepeat--errors.ods",
              [("content.xml", emptyrepeat_ods)],
              mimetype_first="application/vnd.oasis.opendocument.spreadsheet")
    # S1: a DrawingML table cell with an astronomically large span area.
    span_pres = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation {PPTX_NS}>
<p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>
</p:presentation>"""
    span_pres_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"""
    span_slide = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {PPTX_NS}>
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="2" name="T"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">
<a:tbl><a:tr>
<a:tc gridSpan="2000000000" rowSpan="2000000000"><a:txBody><a:bodyPr/><a:p><a:r><a:t>x</a:t></a:r></a:p></a:txBody></a:tc>
</a:tr></a:tbl>
</a:graphicData></a:graphic></p:graphicFrame>
</p:spTree></p:cSld></p:sld>"""
    span_ct = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
<Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>"""
    span_root_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"""
    write_zip(a / "hugespan--errors.pptx", [
        ("[Content_Types].xml", span_ct),
        ("_rels/.rels", span_root_rels),
        ("ppt/presentation.xml", span_pres),
        ("ppt/_rels/presentation.xml.rels", span_pres_rels),
        ("ppt/slides/slide1.xml", span_slide),
    ])


# ---------------------------------------------------------------------------

def main():
    skip_office = "--skip-office" in sys.argv
    for sub in ["odt", "docx", "doc", "rtf", "ods", "xlsx", "xls", "csv",
                "odp", "pptx", "ppt", "epub", "pdf", "malformed", "abuse"]:
        (OUT / sub).mkdir(parents=True, exist_ok=True)

    if not skip_office:
        text = SRC / "text.fodt"
        convert_lo(text, "odt", OUT / "odt", "text.odt")
        convert_lo(text, "docx", OUT / "docx", "text.docx")
        convert_lo(text, "doc:MS Word 97", OUT / "doc", "text.doc")
        convert_lo(text, "rtf:Rich Text Format", OUT / "rtf", "text.rtf")
        convert_lo(text, "pdf:writer_pdf_Export", OUT / "pdf", "text.pdf")
        sheet = SRC / "sheet.fods"
        convert_lo(sheet, "ods", OUT / "ods", "sheet.ods")
        convert_lo(sheet, "xlsx:Calc MS Excel 2007 XML", OUT / "xlsx", "sheet.xlsx")
        convert_lo(sheet, "xls:MS Excel 97", OUT / "xls", "sheet.xls")
        convert_lo(sheet, "csv:Text - txt - csv (StarCalc)", OUT / "csv", "sheet.csv")
        pres = SRC / "pres.fodp"
        convert_lo(pres, "odp", OUT / "odp", "pres.odp")
        convert_lo(pres, "pptx:Impress MS PowerPoint 2007 XML", OUT / "pptx", "pres.pptx")
        convert_lo(pres, "ppt:MS PowerPoint 97", OUT / "ppt", "pres.ppt")
        (SRC / "dot.png").write_bytes(DOT_PNG)
        run(["pandoc", str(SRC / "book.md"), "-o", str(OUT / "epub" / "book.epub"),
             "--resource-path", str(SRC)])

    numbering_docx()
    rich_docx()
    inherit_pptx()
    strict_alt_ooxml()
    gaps_ods()
    encoded_docs()
    order_pptx()
    css_links_epub()
    merge_rtf()
    multimaster_ppt()
    sparsenotes_ppt()
    tables_docx()
    outline_docx()
    ole_docx()
    links_pptx()
    manyrefs_docx()
    defaults_odf()
    merged_xlsx()
    hidden_xlsx()
    features_epub()
    bin_rtf()
    csvs()
    malformed()
    abuse()
    print("fixture generation complete")


if __name__ == "__main__":
    main()
