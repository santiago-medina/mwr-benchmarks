// Closure creation and upvalue access.
function counter() { var c = 0; return function () { c++; return c; }; }
var t = 0;
for (var i = 0; i < 20000; i++) { var f = counter(); f(); t += f(); }
if (t !== 40000) throw new Error('t');
