#!/usr/bin/env python3
"""Usage: python3 -I gen_unicode_props.py UCD_DIR OUT.rs
Emit the RegExp Unicode tables: General_Category, Script, Script_Extensions, the
ECMAScript binary properties, properties of strings (v flag) and simple case folding."""
import sys, re, os
D = sys.argv[1]
def path(n): return os.path.join(D, n)
RX = re.compile(r'^([0-9A-F]+)(?:\.\.([0-9A-F]+))?\s*;\s*([^#]+?)\s*(?:#|$)')
def ranges_file(name, key=lambda v: v):
    out = {}
    for line in open(path(name), encoding='utf-8'):
        m = RX.match(line)
        if not m: continue
        a = int(m.group(1), 16); b = int(m.group(2) or m.group(1), 16)
        for k in key(m.group(3)):
            out.setdefault(k, []).append((a, b))
    return out
def merge(r):
    r = sorted(r); out = []
    for a, b in r:
        if out and a <= out[-1][1] + 1: out[-1] = (out[-1][0], max(b, out[-1][1]))
        else: out.append((a, b))
    return out
def complement(r):
    out = []; nxt = 0
    for a, b in merge(r):
        if a > nxt: out.append((nxt, a - 1))
        nxt = b + 1
    if nxt <= 0x10FFFF: out.append((nxt, 0x10FFFF))
    return out
ver = open(path('DerivedCoreProperties.txt'), encoding='utf-8').readline().strip()[2:]
props = {}
gc = ranges_file('DerivedGeneralCategory.txt', lambda v: [v.strip()])
groups = {'L': 'Lu Ll Lt Lm Lo', 'LC': 'Lu Ll Lt', 'M': 'Mn Mc Me', 'N': 'Nd Nl No',
          'P': 'Pc Pd Ps Pe Pi Pf Po', 'S': 'Sm Sc Sk So', 'Z': 'Zs Zl Zp', 'C': 'Cc Cf Cs Co Cn'}
for g, members in groups.items():
    gc[g] = [r for m in members.split() for r in gc.get(m, [])]
for k, v in gc.items(): props['gc=' + k] = merge(v)
sc = ranges_file('Scripts.txt', lambda v: [v.strip()])
# Script names in Scripts.txt are long names; map to short codes via aliases.
alias = {'gc': {}, 'sc': {}}
for line in open(path('PropertyValueAliases.txt'), encoding='utf-8'):
    parts = [p.strip() for p in line.split('#')[0].split(';')]
    if len(parts) >= 3 and parts[0] in ('gc', 'sc'):
        canon = parts[1]
        for name in parts[1:]:
            if name: alias[parts[0]][name] = canon
long_to_short = {}
for name, canon in alias['sc'].items(): long_to_short.setdefault(name, canon)
for k, v in sc.items(): props['sc=' + long_to_short[k]] = merge(v)
props['sc=Zzzz'] = complement([r for v in sc.values() for r in v])
scx = ranges_file('ScriptExtensions.txt', lambda v: v.split())
scx_full = {long_to_short[k]: list(v) for k, v in sc.items()}
# Script_Extensions: code points listed in ScriptExtensions.txt take that set; others keep Script.
listed = merge([r for v in scx.values() for r in v])
def subtract(r, s):
    out = []; s = merge(s)
    for a, b in merge(r):
        cur = a
        for x, y in s:
            if y < cur or x > b: continue
            if x > cur: out.append((cur, x - 1))
            cur = max(cur, y + 1)
            if cur > b: break
        if cur <= b: out.append((cur, b))
    return out
for k in list(scx_full):
    scx_full[k] = subtract(scx_full[k], listed)
for k, v in scx.items():
    scx_full.setdefault(k, []).extend(v)
for k, v in scx_full.items(): props['scx=' + k] = merge(v)
props['scx=Zzzz'] = props['sc=Zzzz']
BINARY = ['ASCII', 'ASCII_Hex_Digit', 'Alphabetic', 'Any', 'Assigned', 'Bidi_Control', 'Bidi_Mirrored',
          'Case_Ignorable', 'Cased', 'Changes_When_Casefolded', 'Changes_When_Casemapped',
          'Changes_When_Lowercased', 'Changes_When_NFKC_Casefolded', 'Changes_When_Titlecased',
          'Changes_When_Uppercased', 'Dash', 'Default_Ignorable_Code_Point', 'Deprecated', 'Diacritic',
          'Emoji', 'Emoji_Component', 'Emoji_Modifier', 'Emoji_Modifier_Base', 'Emoji_Presentation',
          'Extended_Pictographic', 'Extender', 'Grapheme_Base', 'Grapheme_Extend', 'Hex_Digit',
          'IDS_Binary_Operator', 'IDS_Trinary_Operator', 'ID_Continue', 'ID_Start', 'Ideographic',
          'Join_Control', 'Logical_Order_Exception', 'Lowercase', 'Math', 'Noncharacter_Code_Point',
          'Pattern_Syntax', 'Pattern_White_Space', 'Quotation_Mark', 'Radical', 'Regional_Indicator',
          'Sentence_Terminal', 'Soft_Dotted', 'Terminal_Punctuation', 'Unified_Ideograph', 'Uppercase',
          'Variation_Selector', 'White_Space', 'XID_Continue', 'XID_Start']
binary = {}
for f in ['PropList.txt', 'DerivedCoreProperties.txt', 'DerivedBinaryProperties.txt',
          'DerivedNormalizationProps.txt', 'emoji-data.txt']:
    for k, v in ranges_file(f, lambda v: [v.strip()]).items():
        if k in BINARY: binary.setdefault(k, []).extend(v)
binary['Any'] = [(0, 0x10FFFF)]
binary['ASCII'] = [(0, 0x7F)]
binary['Assigned'] = complement(gc['Cn'])
for k in BINARY:
    assert k in binary, k
    props['bin=' + k] = merge(binary[k])
bin_alias = {}
for line in open(path('PropertyAliases.txt'), encoding='utf-8'):
    parts = [p.strip() for p in line.split('#')[0].split(';')]
    if len(parts) >= 2 and parts[1] in BINARY:
        for name in parts: bin_alias[name] = parts[1]
for k in BINARY: bin_alias.setdefault(k, k)
names = []  # (accepted name, canonical key)
for name, canon in alias['gc'].items():
    if 'gc=' + canon in props: names.append(('gc=' + name, 'gc=' + canon))
for name, canon in alias['sc'].items():
    if 'sc=' + canon in props:
        names.append(('sc=' + name, 'sc=' + canon)); names.append(('scx=' + name, 'scx=' + canon))
for name, canon in bin_alias.items(): names.append(('bin=' + name, 'bin=' + canon))
# Properties of strings.
def seqs(fname, wanted):
    out = {}
    for line in open(path(fname), encoding='utf-8'):
        line = line.split('#')[0].strip()
        if not line: continue
        parts = [p.strip() for p in line.split(';')]
        if parts[1] not in wanted: continue
        cps = parts[0]
        if '..' in cps:
            a, b = cps.split('..')
            out.setdefault(parts[1], []).extend([chr(c)] for c in range(int(a, 16), int(b, 16) + 1))
        else:
            out.setdefault(parts[1], []).append([chr(int(x, 16)) for x in cps.split()])
    return out
strings = seqs('emoji-sequences.txt', {'Basic_Emoji', 'Emoji_Keycap_Sequence', 'RGI_Emoji_Modifier_Sequence',
                                       'RGI_Emoji_Flag_Sequence', 'RGI_Emoji_Tag_Sequence'})
strings.update(seqs('emoji-zwj-sequences.txt', {'RGI_Emoji_ZWJ_Sequence'}))
strings['RGI_Emoji'] = [s for k in list(strings) for s in strings[k]]
def lit(s): return '"' + ''.join(c if 0x20 <= ord(c) < 0x7f and c not in '"\\' else '\\u{%X}' % ord(c) for c in s) + '"'
fold = []
for line in open(path('CaseFolding.txt'), encoding='utf-8'):
    parts = [p.strip() for p in line.split('#')[0].split(';')]
    if len(parts) >= 3 and parts[1] in ('C', 'S'):
        fold.append((int(parts[0], 16), int(parts[2], 16)))
fold.sort()
with open(sys.argv[2], 'w') as o:
    o.write(f'//! Generated by gen_unicode_props.py from {ver}: RegExp property and case-folding tables.\n')
    keys = sorted(props)
    flat = []; index = []
    for k in keys:
        index.append((k, len(flat), len(props[k]))); flat.extend(props[k])
    o.write('#[rustfmt::skip]\npub(super) const RANGES: &[(u32, u32)] = &[\n')
    for i in range(0, len(flat), 8): o.write('    ' + ' '.join(f'(0x{a:X}, 0x{b:X}),' for a, b in flat[i:i + 8]) + '\n')
    o.write('];\n#[rustfmt::skip]\n/// Canonical property key, start and length in RANGES (sorted by key).\npub(super) const PROPERTIES: &[(&str, usize, usize)] = &[\n')
    for k, s, n in index: o.write(f'    ("{k}", {s}, {n}),\n')
    o.write('];\n#[rustfmt::skip]\n/// Accepted spelling to canonical key (sorted by spelling).\npub(super) const NAMES: &[(&str, &str)] = &[\n')
    for a, b in sorted(set(names)): o.write(f'    ("{a}", "{b}"),\n')
    o.write('];\n#[rustfmt::skip]\n/// Simple case folding (status C and S), sorted by code point.\npub(super) const FOLD: &[(u32, u32)] = &[\n')
    for i in range(0, len(fold), 8): o.write('    ' + ' '.join(f'(0x{a:X}, 0x{b:X}),' for a, b in fold[i:i + 8]) + '\n')
    o.write('];\n#[rustfmt::skip]\n/// Properties of strings for the v flag.\npub(super) const STRINGS: &[(&str, &[&str])] = &[\n')
    for k in sorted(strings):
        o.write(f'    ("{k}", &[\n')
        uniq = sorted(set(''.join(s) for s in strings[k]))
        for i in range(0, len(uniq), 6): o.write('        ' + ' '.join(lit(s) + ',' for s in uniq[i:i + 6]) + '\n')
        o.write('    ]),\n')
    o.write('];\n')
print(ver, 'properties', len(props), 'ranges', len(flat), 'names', len(set(names)), 'fold', len(fold),
      {k: len(set(''.join(s) for s in v)) for k, v in strings.items()})
