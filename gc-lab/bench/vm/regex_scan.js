// RegExp over a long built string (property-escape shape, regex half only).
var a = [];
for (var cp = 0x4E00; cp < 0x4E00 + 20000; cp++) a.push(cp);
var s = String.fromCodePoint.apply(null, a);
if (!/^\p{Script=Han}+$/u.test(s)) throw new Error('re');
