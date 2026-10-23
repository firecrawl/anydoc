"""Build small original CHM fixtures; no vendor manual content is included.

Run from any directory: python3 tests/fixture-src/chm.py
"""

from pathlib import Path
import struct
from urllib.parse import quote_from_bytes


def encint(value):
    groups = [value & 0x7f]
    while value >> 7:
        value >>= 7
        groups.append((value & 0x7f) | 0x80)
    return bytes(reversed(groups))


def archive(parts, compressed=False):
    """ITSF v3 with a single PMGL directory and optional LZX content."""
    data = bytearray()
    entries = []
    for name, content in sorted(parts.items(), key=lambda item: item[0].lower()):
        entries.append(("/" + name.lstrip("/"), int(compressed), len(data), len(content)))
        data += content
    if compressed:
        assert len(data) <= 32768
        # A valid LZX uncompressed block inside the compressed data space.
        # Exercise CHM framing/reset metadata without an external CHM compiler.
        bits = (3 << 28) | (len(data) << 4)
        stream = struct.pack("<HHIII", bits >> 16, bits & 0xffff, 1, 1, 1) + data
        if len(data) % 2:
            stream += b"\0"
        control = struct.pack("<I4sIIIII", 6, b"LZXC", 2, 2, 2, 1, 0)
        table = struct.pack("<IIIIQQQQ", 2, 1, 8, 40, len(data), len(stream), 32768, 0)
        prefix = "::DataSpace/Storage/MSCompressed/"
        entries += [
            (prefix + "ControlData", 0, 0, len(control)),
            (prefix + "Transform/{7FC28940-9D31-11D0-9B27-00A0C91E9C7C}/InstanceData/ResetTable", 0, len(control), len(table)),
            (prefix + "Content", 0, len(control) + len(table), len(stream)),
        ]
        data = control + table + stream
    return container(entries, data)


def container(entries, data):
    """ITSF v3 container for stored entries and caller-built compressed data."""
    directory = bytearray()
    for name, space, start, length in sorted(entries, key=lambda item: item[0].lower()):
        name = name.encode("utf-8")
        directory += encint(len(name)) + name + encint(space) + encint(start) + encint(length)
    block_size = 4096
    assert len(directory) + 22 < block_size
    pmgl = b"PMGL" + struct.pack("<IIii", block_size - 20 - len(directory), 0, -1, -1)
    pmgl += directory
    pmgl += bytes(block_size - len(pmgl) - 2) + struct.pack("<H", len(entries))
    guid = bytes.fromhex("6a92025d2e21d0119df900a0c922e6ec")
    itsp = b"ITSP" + struct.pack("<IIIIIIiiiiiI", 1, 0x54, 10, block_size, 2, 1, -1, 0, 0, -1, 1, 0x804)
    itsp += guid + struct.pack("<IIII", 0x54, 0xffffffff, 0xffffffff, 0xffffffff)
    assert len(itsp) == 0x54
    section0 = struct.pack("<IIQII", 0x1fe, 0, 0, 0, 0)
    directory_offset = 0x60 + len(section0)
    directory_size = len(itsp) + len(pmgl)
    data_offset = directory_offset + directory_size
    itsf = b"ITSF" + struct.pack("<IIIII", 3, 0x60, 1, 0, 0x804)
    itsf += bytes.fromhex("10fd017caa7bd0119e0c00a0c922e6ec") + bytes.fromhex("11fd017caa7bd0119e0c00a0c922e6ec")
    itsf += struct.pack("<QQQQQ", 0x60, len(section0), directory_offset, directory_size, data_offset)
    assert len(itsf) == 0x60
    return itsf + section0 + itsp + pmgl + data


class Bits:
    """MSB-first LZX bits, serialized as little-endian 16-bit words."""

    def __init__(self):
        self.bits = []

    def push(self, value, count):
        self.bits.extend((value >> shift) & 1 for shift in range(count - 1, -1, -1))

    def finish(self):
        bits = self.bits + [0] * (-len(self.bits) % 16)
        data = bytearray()
        for start in range(0, len(bits), 16):
            word = 0
            for bit in bits[start:start + 16]:
                word = (word << 1) | bit
            data += struct.pack("<H", word)
        return bytes(data)

    def tree_lengths(self, lengths):
        # Complete canonical pretree: 12 codes of length 4, 8 of length 5.
        for symbol in range(20):
            self.push(4 if symbol < 12 else 5, 4)
        for length in lengths:
            delta = (17 - length) % 17
            self.push(delta if delta < 12 else 24 + delta - 12, 4 if delta < 12 else 5)


def huffman_group(literal):
    """Two frames sharing one verbatim Huffman block and its dictionary."""
    bits = Bits()
    bits.push(0, 1)  # No E8 translation; present at each reset boundary.
    bits.push(1, 3)  # Verbatim Huffman block, not an LZX uncompressed block.
    bits.push(65536, 24)
    main = [0] * 512  # 64 KiB window: 32 position slots.
    for symbol in [ord("A"), ord("B"), 261, 262]:
        main[symbol] = 2  # Two literals, R0 length-7 and length-8 matches.
    bits.tree_lengths(main[:256])
    bits.tree_lengths(main[256:])
    bits.tree_lengths([0] * 249)
    bits.push(literal - ord("A"), 2)
    for _ in range(4095):
        bits.push(3, 2)
    bits.push(2, 2)
    first = bits.finish()
    bits = Bits()
    for _ in range(4096):
        bits.push(3, 2)
    return first + bits.finish()


def huffman_spaninfo():
    """Four compressed frames, with a dictionary reset after the second."""
    stream = huffman_group(ord("A")) + huffman_group(ord("B"))
    # Version 2: reset and window units are 32 KiB. The unknown field is 2
    # deliberately: it must not extend the two-frame reset interval.
    control = struct.pack("<I4sIIIII", 6, b"LZXC", 2, 2, 2, 2, 0)
    span = struct.pack("<Q", 131072)
    prefix = "::DataSpace/Storage/MSCompressed/"
    return container([
        ("/page.html", 1, 0, 131072),
        (prefix + "ControlData", 0, 0, len(control)),
        (prefix + "SpanInfo", 0, len(control), len(span)),
        (prefix + "Content", 0, len(control) + len(span), len(stream)),
    ], control + span + stream)


def system_record(kind, value):
    return struct.pack("<HH", kind, len(value)) + value


def system(lcid=0x0804, toc=None, compiled=None):
    records = struct.pack("<I", 3) + system_record(4, struct.pack("<I", lcid))
    if toc:
        records += system_record(0, toc.encode("ascii") + b"\0")
    if compiled:
        records += system_record(6, compiled.encode("ascii") + b"\0")
    return records


def regressions(dest, png):
    """Small books whose links and presentation depend on legacy metadata."""
    chinese_topic = quote_from_bytes("中文.htm".encode("gbk"))
    chinese_fragment = quote_from_bytes("参数".encode("gbk"))
    (dest / "handmade-legacy-links.chm").write_bytes(archive({
        "#SYSTEM": system(toc="contents.hhc"),
        # Both this TOC and the source page are intentionally ASCII-only and
        # contain no charset declaration. Their URL octets use the book LCID.
        "contents.hhc": (
            '<ul><li><object type="text/sitemap"><param name="Name" value="First topic">'
            f'<param name="Local" value="{chinese_topic}"></object>'
            '<li><object type="text/sitemap"><param name="Name" value="Second topic">'
            '<param name="Local" value="a-intro.htm"></object></ul>'
        ).encode("ascii"),
        "a-intro.htm": (
            f'<h1>Second topic</h1><p><a href="{chinese_topic}#{chinese_fragment}">Legacy link</a>'
        ).encode("ascii"),
        "中文.htm": '<meta charset="gbk"><h1>First topic</h1><h2 id="参数">参数</h2>'.encode("gbk"),
    }))
    css_path = quote_from_bytes("隱藏.css".encode("big5"))
    (dest / "handmade-legacy-css.chm").write_bytes(archive({
        "#SYSTEM": system(),
        "path.htm": (
            f'<meta charset="big5"><link rel="stylesheet" href="styles/{css_path}">'
            '<h1>Page encoded CSS path</h1><p class="隱藏">HIDDEN-PATH</p><p>VISIBLE-PATH</p>'
        ).encode("big5"),
        "declared.htm": (
            '<meta charset="big5"><link rel="stylesheet" href="styles/declared.css">'
            '<h1>CSS declared encoding</h1><p class="隱藏">HIDDEN-CHARSET</p><p>VISIBLE-CHARSET</p>'
        ).encode("big5"),
        "styles/隱藏.css": '.隱藏 { display: none; }'.encode("big5"),
        "styles/declared.css": '@charset "big5"; .隱藏 { display: none; }'.encode("big5"),
    }))
    (dest / "handmade-polish.chm").write_bytes(archive({
        "#SYSTEM": system(lcid=0x0415),
        "topic.htm": (
            '<title>Command manual</title><h2>Składnia</h2><p>Zażółć gęślą jaźń.</p>'
        ).encode("cp1250"),
    }))
    parts = {
        "#SYSTEM": system(compiled="handmade-base.chm"),
        "shared/target.htm": b'<h1 id="dest">Base target</h1>',
        "shared/dot.png": png,
        "shared/hidden.css": b'.hidden { display: none; }',
    }
    for kind, base in [
        ("relative", "../shared/"),
        ("root", "/shared/"),
        ("same-book", "ms-its:handmade-base.chm::/shared/"),
        ("external", "https://example.invalid/shared/"),
    ]:
        parts[f"topics/{kind}.htm"] = (
            f'<head><base href="{base}"><link rel="stylesheet" href="hidden.css"></head>'
            f'<body><h1 id="self">Base {kind}</h1>'
            f'<p><a href="target.htm#dest">Link {kind}</a>'
            f'<img src="dot.png" alt="Image {kind}"></p>'
            f'<p class="hidden">HIDDEN-{kind}</p>'
            f'<p><a href="#self">Fragment {kind}</a></p></body>'
        ).encode("ascii")
    (dest / "handmade-base.chm").write_bytes(archive(parts))


def main():
    root = Path(__file__).resolve().parents[1]
    dest = root / "fixtures" / "chm"
    dest.mkdir(parents=True, exist_ok=True)
    system = struct.pack("<I", 3)
    system += system_record(0, b"contents.hhc\0")
    system += system_record(2, b"z-intro.htm\0")
    system += system_record(3, b"CHM test book\0")
    toc = """<HTML><BODY><UL>
    <LI><OBJECT type="text/sitemap"><PARAM name="Name" value="使用指南"><PARAM name="Local" value="z-intro.htm"></OBJECT>
    <UL><LI><OBJECT type="text/sitemap"><PARAM name="Name" value="命令参考"><PARAM name="Local" value="a-commands.htm"></OBJECT></UL>
    <LI><OBJECT type="text/sitemap"><PARAM name="Name" value="再次阅读"><PARAM name="Local" value="z-intro.htm#overview"></OBJECT>
    </UL></BODY></HTML>"""
    intro = """<HTML><HEAD><META charset="gbk"><TITLE>使用指南</TITLE></HEAD><BODY>
    <H1 id="overview">使用指南</H1><P>中文说明：配置路由器
    <P><A HREF="a-commands.htm#syntax">命令语法</A><BR><IMG SRC="images/dot.png" ALT="拓扑图">
    <UL><LI>第一步<LI>第二步</UL></BODY></HTML>"""
    commands = """<html><head><meta charset="big5"><title>命令参考</title></head><body>
    <h1 id="overview">命令参考</h1><h2 id="syntax">語法</h2><pre>show version\nshow ip</pre>
    <table><tr><th>命令<th>说明<tr><td>show version<td>显示版本</table>
    <p><a href="z-intro.htm#overview">返回指南</a><img src="images/dot.png" alt="拓扑图"></body></html>"""
    # Big5 does not encode all simplified characters: use GBK for the shared
    # book, and keep a separate Big5 fixture for explicit traditional text.
    commands = commands.replace('charset="big5"', 'charset="gbk"')
    parts = {
        "#SYSTEM": system,
        "contents.hhc": b'<meta charset="gbk">' + toc.encode("gbk"),
        "z-intro.htm": intro.encode("gbk"),
        "a-commands.htm": commands.encode("gbk"),
        "extra.htm": '<meta charset="utf-8"><h1>补充说明</h1><p>未列出的主题'.encode(),
        "images/dot.png": (root / "fixture-src" / "dot.png").read_bytes(),
    }
    (dest / "handmade-book.chm").write_bytes(archive(parts))
    (dest / "handmade-compressed.chm").write_bytes(archive(parts, compressed=True))
    (dest / "handmade-big5.chm").write_bytes(archive({
        "topic.htm": ('<meta charset="big5"><h1>繁體中文</h1><p>路由器設定與命令參考'
                      '<p><a href="#%B0%D1%BC%C6">參數連結</a><h2 id="參數">參數</h2>').encode("big5"),
    }))
    (dest / "handmade-no-toc.chm").write_bytes(archive({
        "b.htm": b"<H1>Second</H1><P>second topic",
        "a.htm": b"<H1>First</H1><P>first topic",
    }))
    regressions(dest, (root / "fixture-src" / "dot.png").read_bytes())
    (dest / "handmade-huffman-spaninfo.chm").write_bytes(huffman_spaninfo())


if __name__ == "__main__":
    main()
