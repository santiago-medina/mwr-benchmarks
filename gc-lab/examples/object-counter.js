const model = {
  _count: 0,
  increment: function () { this.count = this.count + 1; },
  get label() { return 'Cuenta: ' + String(this.count); }
};
Object.defineProperty(model, 'count', {
  get: function () { return this._count; },
  set: function (value) {
    if (value < 0) { throw new RangeError('negative count'); }
    this._count = value;
  },
  enumerable: true
});
const button = document.querySelector('#button');
button.addEventListener('click', function () {
  try {
    model.increment();
    this.textContent = model.label;
  } catch (error) {
    this.textContent = error.message;
  }
});
button.textContent = model.label;
