// String concat + charCodeAt + method calls.
var s = '';
for (var i = 0; i < 20000; i++) s += String.fromCharCode(97 + (i % 26));
var h = 0;
for (var j = 0; j < s.length; j++) h = (h * 31 + s.charCodeAt(j)) | 0;
if (typeof h !== 'number') throw new Error('h');
