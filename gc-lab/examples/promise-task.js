const button = document.querySelector('#button');
const result = document.querySelector('#result');
button.addEventListener('click', function () {
  result.textContent = 'Pendiente';
  const payload = document.createElement('span');
  payload.textContent = 'conservado por la microtarea';
  const order = [];
  Promise.resolve({then: function (resolve) {
    order.push('adoptar');
    resolve(payload);
  }}).then(function (node) {
    order.push('then');
    if (node.textContent !== 'conservado por la microtarea') { throw new Error('GC perdió el nodo'); }
    return 4;
  }).finally(function () {
    order.push('finally');
  }).then(function (value) {
    result.textContent = 'Resultado ' + String(value) + ' | ' + order.join(' > ');
  }).catch(function (error) {
    result.textContent = 'Error: ' + error.message;
  });
  queueMicrotask(function () { order.push('micro'); });
});
