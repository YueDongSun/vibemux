'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');
const merge = require('../index.cjs');

function assert_safe_result(result) {
  assert.equal(Object.getPrototypeOf(result), Object.prototype);
  assert.equal(result.is_admin, undefined);
  assert.equal(Object.hasOwn(result, '__proto__'), false);
  assert.equal(Object.hasOwn(result, 'constructor'), false);
  assert.equal(Object.hasOwn(result, 'prototype'), false);
  assert.equal({}.is_admin, undefined);
}

test('malicious source cannot poison a null-prototype target result', () => {
  const source = JSON.parse('{"__proto__":{"is_admin":true},"safe":1}');
  const result = merge(Object.create(null), source);
  assert_safe_result(result);
  assert.equal(result.safe, 1);
});

test('nested null and primitive placeholders do not enable prototype poisoning', () => {
  for (const placeholder of [null, undefined, 0, false, '']) {
    const source = JSON.parse('{"session":{"__proto__":{"is_admin":true},"safe":2}}');
    const result = merge({ session: placeholder }, source);
    assert_safe_result(result.session);
    assert.equal(result.session.safe, 2);
  }
});

test('own forbidden keys on the target are rejected before copying', () => {
  const target = JSON.parse('{"__proto__":{"is_admin":true},"constructor":3,"prototype":4,"safe":5}');
  const result = merge(target, {});
  assert_safe_result(result);
  assert.equal(result.safe, 5);
});

test('forbidden source keys are rejected regardless of target shape', () => {
  const source = JSON.parse('{"__proto__":{"is_admin":true},"constructor":3,"prototype":4}');
  for (const target of [{}, Object.create(null), { constructor: null, prototype: null }]) {
    assert_safe_result(merge(target, source));
  }
});

test('throwing property-presence checks fail closed', () => {
  const target = new Proxy({}, { has() { throw new Error('untrusted presence trap'); } });
  assert.deepEqual(merge(target, { added: 1 }), {});
});

test('copying an allowed key does not invoke an inherited destination setter', () => {
  let setter_calls = 0;
  let result;
  Object.defineProperty(Object.prototype, 'merge_test_setter', {
    configurable: true,
    set() { setter_calls += 1; },
  });
  try {
    result = merge(Object.create(null), { merge_test_setter: 7 });
  } finally {
    delete Object.prototype.merge_test_setter;
  }
  assert.equal(setter_calls, 0);
  assert.equal(Object.hasOwn(result, 'merge_test_setter'), true);
  assert.equal(result.merge_test_setter, 7);
});

test('deep object merging and array concatenation clone without mutating inputs', () => {
  const target = { nested: { left: 1 }, entries: [{ first: 1 }] };
  const source = { nested: { right: 2 }, entries: [{ second: 2 }] };
  const result = merge(target, source);
  assert.deepEqual(result, { nested: { left: 1, right: 2 }, entries: [{ first: 1 }, { second: 2 }] });
  result.entries[0].first = 9;
  assert.equal(target.entries[0].first, 1);
  assert.deepEqual(source, { nested: { right: 2 }, entries: [{ second: 2 }] });
});

test('custom array merging receives the documented clone helper', () => {
  const source = [{ value: 2 }];
  const result = merge({ entries: [1] }, { entries: source }, {
    arrayMerge(_target, values, options) {
      return values.map(value => options.cloneUnlessOtherwiseSpecified(value, options));
    },
  });
  assert.deepEqual(result, { entries: [{ value: 2 }] });
  assert.notEqual(result.entries[0], source[0]);
});

test('customMerge and merge.all retain their public API behavior', () => {
  const result = merge({ entries: [1] }, { entries: [2] }, {
    customMerge(key) {
      return key === 'entries' ? (left, right) => right.concat(left) : undefined;
    },
  });
  assert.deepEqual(result, { entries: [2, 1] });
  assert.deepEqual(merge.all([{ nested: { first: 1 } }, { nested: { second: 2 } }]),
    { nested: { first: 1, second: 2 } });
  assert.throws(() => merge.all({}), /first argument should be an array/);
});

test('clone false and custom isMergeableObject preserve opaque values', () => {
  const opaque = { value: 1 };
  assert.equal(merge({}, { opaque }, { clone: false }).opaque, opaque);
  class Opaque { constructor() { this.value = 1; } }
  const instance = new Opaque();
  const result = merge({}, { instance }, {
    isMergeableObject(value) { return value !== null && Object.getPrototypeOf(value) === Object.prototype; },
  });
  assert.equal(result.instance, instance);
});

test('enumerable symbols merge while dates, regexps and React elements remain opaque', () => {
  const key = Symbol('config');
  const date = new Date(0);
  const pattern = /value/;
  const react_element = { $$typeof: Symbol.for('react.element'), type: 'example' };
  const result = merge({ [key]: { first: 1 } }, { [key]: { second: 2 }, date, pattern, react_element });
  assert.deepEqual(result[key], { first: 1, second: 2 });
  assert.equal(result.date, date);
  assert.equal(result.pattern, pattern);
  assert.equal(result.react_element, react_element);
});

test('ordinary inherited properties remain excluded', () => {
  const source = Object.create({ inherited: true });
  source.own = 1;
  assert.deepEqual(merge({}, source), { own: 1 });
  assert.deepEqual(merge({ nested: 1 }, ['value']), ['value']);
});
