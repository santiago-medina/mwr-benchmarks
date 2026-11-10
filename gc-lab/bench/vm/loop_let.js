// buildString shape: counted loop, let bindings, array index store, apply in chunks.
var s = '';
for (let i = 0; i < 1; i++) {
  let codePoints = [];
  for (let length = 0, cp = 0; cp <= 200000; cp++) {
    codePoints[length++] = cp;
    if (length === 10000) { s += String.fromCodePoint.apply(null, codePoints); codePoints.length = length = 0; }
  }
}
if (s.length < 200000) throw new Error('bad ' + s.length);
