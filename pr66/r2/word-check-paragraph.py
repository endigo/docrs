"""Writes word-check-paragraph.docx: the paragraph constructs of the PDF painter, in Arial 11 pt,
with a styles.xml whose defaults are single spacing and no space after (so y positions compare)."""
import sys,zipfile
W='http://schemas.openxmlformats.org/wordprocessingml/2006/main'
def r(text='',rpr='',tab=False,br=False):
    inner=''.join(['<w:tab/>' if tab else '','<w:br/>' if br else '',f'<w:t xml:space="preserve">{text}</w:t>' if text else ''])
    return f'<w:r><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial"/><w:sz w:val="22"/>{rpr}</w:rPr>{inner}</w:r>'
def p(runs,ppr=''):
    return f'<w:p><w:pPr><w:spacing w:after="120"/>{ppr}</w:pPr>{"".join(runs)}</w:p>'
def tabbed(text,stops,ppr=''):
    parts=text.split('\t'); runs=[r(parts[0])]
    for part in parts[1:]: runs+= [r(tab=True), r(part)]
    return p(runs,f'<w:tabs>{stops}</w:tabs>{ppr}')
def head(text): return p([r(text,'<w:b/>')],'<w:spacing w:before="240" w:after="60"/>')
lorem=("Justified text spreads the space left on each line over its gaps between words, so both edges "
       "line up with the margins; the last line of the paragraph keeps its natural spacing. ")
body=[]
body.append(head('1. Justified (jc both), the second sentence after a line break'))
body.append(p([r(lorem*2),r('A short line',br=False),r(br=True),r('after a line break, then more text to fill the paragraph so that it wraps over several lines as well.')],'<w:jc w:val="both"/>'))
body.append(head('2. Leaders on a right tab at 6 in: dot, hyphen, underscore, heavy, middleDot'))
for leader in ['dot','hyphen','underscore','heavy','middleDot']:
    body.append(tabbed(f'Entry with {leader}\t12',f'<w:tab w:val="right" w:leader="{leader}" w:pos="8640"/>'))
body.append(p([r('Entry, tab run spaced 2 pt'),r(tab=True,rpr='<w:spacing w:val="40"/>'),r('12')],'<w:tabs><w:tab w:val="right" w:leader="dot" w:pos="8640"/></w:tabs>'))
body.append(head('3. Decimal tab at 3 in'))
for row in ['Rent\t1,250.00','Water\t42.5','Count\t7','Total\t1,299.50']:
    body.append(tabbed(row,'<w:tab w:val="decimal" w:pos="4320"/>','<w:spacing w:after="0"/>'))
body.append(head('4. Bar tab at 1 in, left tab at 2 in'))
body.append(tabbed('Item\tA note after the bar tab, long enough to wrap onto a second line so that the bar shows on both lines of the paragraph.','<w:tab w:val="bar" w:pos="1440"/><w:tab w:val="left" w:pos="2880"/>'))
body.append(head('5. Tabs past the right margin (text width 6.5 in)'))
body.append(tabbed('Left stop at 7 in\tafter','<w:tab w:val="left" w:pos="10080"/>'))
body.append(tabbed('Right stop at 7 in\tafter','<w:tab w:val="right" w:pos="10080"/>'))
body.append(tabbed('A line whose next default stop lies past the right margin, then a tab\tafter',''))
body.append(head('6. List labels: lvlJc left, center, right; suffix tab, space, nothing; bold red 14 pt'))
for num in (1,2,3):
    for item in range(4):
        body.append(p([r(f'Item {item+1} of list {num}')],f'<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{num}"/></w:numPr><w:spacing w:after="0"/>'))
body.append(head('7. Run formatting'))
body.append(p([r('Small Caps Sample','<w:smallCaps/>'),r(' then '),r('All Caps Sample','<w:caps/>')]))
body.append(p([r('Spaced by 2 pt','<w:spacing w:val="40"/>'),r(' and '),r('condensed by 1 pt','<w:spacing w:val="-20"/>')]))
body.append(p([r('Scaled 150%','<w:w w:val="150"/>'),r(' and '),r('scaled 66%','<w:w w:val="66"/>')]))
body.append(p([r('Raised 3 pt','<w:position w:val="6"/>'),r(' and '),r('lowered 3 pt','<w:position w:val="-6"/>'),r(' and x'),r('2','<w:vertAlign w:val="superscript"/>'),r(' and H'),r('2','<w:vertAlign w:val="subscript"/>'),r('O')]))
body.append(p([r('AV WA To Ta kerned from 8 pt','<w:kern w:val="16"/>'),r(' / '),r('AV WA To Ta not kerned')]))
sect='<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>'
doc=f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}"><w:body>{"".join(body)}{sect}</w:body></w:document>'
def level(jc,suff):
    return (f'<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="upperRoman"/><w:lvlText w:val="%1."/>'
            f'<w:suff w:val="{suff}"/><w:lvlJc w:val="{jc}"/><w:pPr><w:ind w:left="1440" w:hanging="720"/></w:pPr>'
            '<w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial"/><w:b/><w:color w:val="C00000"/><w:sz w:val="28"/></w:rPr></w:lvl>')
nums=''.join(f'<w:abstractNum w:abstractNumId="{i}">{level(jc,suff)}</w:abstractNum>' for i,(jc,suff) in enumerate([('left','tab'),('center','space'),('right','nothing')]))
nums+=''.join(f'<w:num w:numId="{i+1}"><w:abstractNumId w:val="{i}"/></w:num>' for i in range(3))
numbering=f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering xmlns:w="{W}">{nums}</w:numbering>'
styles=(f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="{W}"><w:docDefaults>'
 '<w:rPrDefault><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:eastAsia="Arial" w:cs="Arial"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault>'
 '<w:pPrDefault><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>'
 '<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style></w:styles>')
settings=f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings xmlns:w="{W}"><w:defaultTabStop w:val="720"/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>'
types=('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
 '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>'
 '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
 '<Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>'
 '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
 '<Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/></Types>')
rels='<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>'
drels=('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
 '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>'
 '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>'
 '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>')
with zipfile.ZipFile(sys.argv[1],'w',zipfile.ZIP_DEFLATED) as z:
    for name,data in [('[Content_Types].xml',types),('_rels/.rels',rels),('word/_rels/document.xml.rels',drels),('word/document.xml',doc),('word/numbering.xml',numbering),('word/settings.xml',settings),('word/styles.xml',styles)]:
        z.writestr(name,data)
