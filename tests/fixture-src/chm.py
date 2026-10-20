"""Build small original CHM fixtures; no vendor manual content is included.

Run from any directory: python3 tests/fixture-src/chm.py
"""

from pathlib import Path
import struct


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


def system_record(kind, value):
    return struct.pack("<HH", kind, len(value)) + value


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


if __name__ == "__main__":
    main()
