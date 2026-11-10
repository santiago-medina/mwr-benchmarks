// Allocation + named property get/set.
var sum = 0;
for (var i = 0; i < 50000; i++) { var o = { x: i, y: i + 1 }; o.z = o.x + o.y; sum += o.z; }
if (sum !== 2500000000) throw new Error('sum ' + sum);
