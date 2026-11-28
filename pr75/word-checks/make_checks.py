"""Writes pictures-check.docx and cover-check.docx: generated Word checks for the PDF painter's pictures."""
import sys, zipfile

OUT = sys.argv[1]
CHART = open(sys.argv[2], 'rb').read()
EMU = 12700
NS = ('xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" '
      'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
      'xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" '
      'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
      'xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"')
ids = iter(range(1, 1000))

def pic(w, h, fill='', xfrm=''):
    n = next(ids)
    return (f'<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic>'
            f'<pic:nvPicPr><pic:cNvPr id="{n}" name="chart{n}.png"/><pic:cNvPicPr/></pic:nvPicPr>'
            f'<pic:blipFill><a:blip r:embed="rIdChart"/>{fill}<a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
            f'<pic:spPr><a:xfrm {xfrm}><a:off x="0" y="0"/><a:ext cx="{int(w*EMU)}" cy="{int(h*EMU)}"/></a:xfrm>'
            f'<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>'), n

def inline(w, h, fill='', xfrm='', effect=(0, 0, 0, 0)):
    g, n = pic(w, h, fill, xfrm)
    l, t, r, b = (int(v*EMU) for v in effect)
    return (f'<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{int(w*EMU)}" cy="{int(h*EMU)}"/>'
            f'<wp:effectExtent l="{l}" t="{t}" r="{r}" b="{b}"/><wp:docPr id="{n}" name="Picture {n}"/>'
            f'<wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr>{g}</wp:inline></w:drawing></w:r>')

def anchor(w, h, h_pos, v_pos, behind=False, order=1):
    g, n = pic(w, h)
    return (f'<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="{order}" '
            f'behindDoc="{1 if behind else 0}" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/>'
            f'{h_pos}{v_pos}<wp:extent cx="{int(w*EMU)}" cy="{int(h*EMU)}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/>'
            f'<wp:docPr id="{n}" name="Picture {n}"/><wp:cNvGraphicFramePr/>{g}</wp:anchor></w:drawing></w:r>')

def offset(axis, frm, pt):
    return f'<wp:position{axis} relativeFrom="{frm}"><wp:posOffset>{int(pt*EMU)}</wp:posOffset></wp:position{axis}>'

def aligned(axis, frm, how):
    return f'<wp:position{axis} relativeFrom="{frm}"><wp:align>{how}</wp:align></wp:position{axis}>'

def text(t):
    return f'<w:r><w:t xml:space="preserve">{t}</w:t></w:r>'

def para(content, ppr=''):
    return f'<w:p><w:pPr>{ppr}</w:pPr>{content}</w:p>'

STYLES = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles ' + NS + '><w:docDefaults><w:rPrDefault><w:rPr>'
          '<w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial"/><w:sz w:val="22"/></w:rPr></w:rPrDefault>'
          '<w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>'
          '<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style></w:styles>')
SECTION = '<w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1080" w:left="1800" w:header="720" w:footer="720" w:gutter="0"/>'

def package(path, body, headers=()):
    types = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
             '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
             '<Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/>'
             '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
             '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>')
    rels = ('<Relationship Id="rIdChart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/chart.png"/>'
            '<Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>')
    with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as z:
        for name, content in headers:
            types += f'<Override PartName="/word/{name}" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/>'
            rels += f'<Relationship Id="rId{name}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="{name}"/>'
            z.writestr(f'word/{name}', f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr {NS}>{content}</w:hdr>')
            z.writestr(f'word/_rels/{name}.rels', '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdChart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/chart.png"/></Relationships>')
        z.writestr('[Content_Types].xml', types + '</Types>')
        z.writestr('_rels/.rels', '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>')
        z.writestr('word/document.xml', f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{body}</w:body></w:document>')
        z.writestr('word/_rels/document.xml.rels', f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>')
        z.writestr('word/styles.xml', STYLES)
        z.writestr('word/media/chart.png', CHART)

words = ('Words run under and over the anchored pictures so the layers show which is drawn first. ' * 6).strip()
filler = ('This paragraph fills the page so the next one starts near its foot. ' * 14).strip()
crossing = ('The anchor at the start of this paragraph is placed 0 pt below the paragraph, and the one in its middle the same way. ' * 9).strip()
half = len(crossing) // 2
body = (
    para(text('1. Inline: plain, cropped (l 25%, t 10%, r 25%, b 30%), rotated 90°, flipped horizontally, flipped vertically and turned 180°.'))
    + para(inline(80, 40) + text(' ') + inline(80, 40, '<a:srcRect l="25000" t="10000" r="25000" b="30000"/>')
           + text(' ') + inline(80, 40, xfrm='rot="5400000"', effect=(20, 0, 20, 0)) + text(' ')
           + inline(80, 40, xfrm='flipH="1"') + text(' ') + inline(80, 40, xfrm='flipV="1" rot="10800000"'))
    + para(text('2. Layers: the left picture is behind the text (behindDoc), the right one in front, both relative to this paragraph.'))
    + para(anchor(160, 80, offset('H', 'margin', 0), offset('V', 'paragraph', 0), behind=True, order=2)
           + anchor(160, 80, aligned('H', 'margin', 'right'), offset('V', 'paragraph', 10), order=3)
           + text(words))
    + para(text('3. Frames: page (36, 36), right margin centre, bottom margin top.'))
    + para(anchor(40, 20, offset('H', 'page', 36), offset('V', 'page', 36), order=4)
           + anchor(40, 20, aligned('H', 'rightMargin', 'center'), aligned('V', 'margin', 'center'), order=5)
           + anchor(40, 20, aligned('H', 'margin', 'left'), aligned('V', 'bottomMargin', 'top'), order=6)
           + text(filler))
    + para(text(filler))
    + para(text('4. Across a page break:'))
    + para(anchor(60, 30, aligned('H', 'margin', 'right'), offset('V', 'paragraph', 0), order=7)
           + text(crossing[:half])
           + anchor(60, 30, aligned('H', 'margin', 'left'), offset('V', 'paragraph', 0), order=8)
           + text(crossing[half:]))
    + f'<w:sectPr>{SECTION}</w:sectPr>'
)
package(f'{OUT}/pictures-check.docx', body)

background = para(anchor(632, 812, offset('H', 'page', -10), offset('V', 'page', -10), behind=True, order=1))
cover_body = (para(text('Cover title over a full-page background'), '<w:jc w:val="center"/><w:spacing w:before="2400"/>')
              + para('<w:r><w:br w:type="page"/></w:r>' + text('Page 2 has no background.'))
              + f'<w:sectPr><w:headerReference w:type="first" r:id="rIdheader1.xml"/><w:titlePg/>{SECTION}</w:sectPr>')
package(f'{OUT}/cover-check.docx', cover_body, [('header1.xml', background)])
