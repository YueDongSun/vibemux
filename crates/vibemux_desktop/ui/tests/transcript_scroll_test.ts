import assert from 'node:assert/strict';
import test, { type TestContext } from 'node:test';
import { reveal_transcript_element } from '../src/lib/transcript_scroll.ts';

class MockElement {
  parentElement: MockElement | null = null;
  marked = false;
  top = 0;
  bottom = 20;
  scroll_into_view_calls = 0;
  hasAttribute(name: string): boolean {
    assert.equal(name, 'data-transcript-scroll');
    return this.marked;
  }
  getBoundingClientRect(): DOMRect {
    return { top: this.top, bottom: this.bottom, height: this.bottom - this.top } as DOMRect;
  }
  scrollIntoView(): void {
    this.scroll_into_view_calls++;
    throw new Error('outer scrolling is forbidden');
  }
}
class MockHtmlElement extends MockElement {
  clientTop = 0;
  clientHeight = 200;
  scrollHeight = 1200;
  scrollTop = 0;
  scrollLeft = 37;
  calls: ScrollToOptions[] = [];
  scrollTo(options: ScrollToOptions): void {
    this.calls.push({ ...options });
    if (options.top !== undefined) this.scrollTop = options.top;
    if (options.left !== undefined) this.scrollLeft = options.left;
  }
}
function html_constructor(context: TestContext): void {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'HTMLElement');
  Object.defineProperty(globalThis, 'HTMLElement', { configurable: true, value: MockHtmlElement });
  context.after(() => {
    if (previous) Object.defineProperty(globalThis, 'HTMLElement', previous);
    else Reflect.deleteProperty(globalThis, 'HTMLElement');
  });
}
const element = (value: MockElement): Element => value as unknown as Element;

test('Find centers the selected text range inside a long diff instead of the whole text block', context => {
  html_constructor(context);
  const container = root(), target = new MockHtmlElement();
  target.parentElement = container; target.top = 0; target.bottom = 20_000;
  let range_reads = 0;
  const selection = { getBoundingClientRect() { range_reads++; return { top: 400, bottom: 420, height: 20 } as DOMRect; } } as Range;
  assert.equal(reveal_transcript_element(element(target), selection), true);
  assert.equal(range_reads, 1);
  assert.deepEqual(container.calls, [{ top: 310, behavior: 'instant' }]);
});

test('Find opens only disclosure ancestors inside the nearest transcript before measuring its hidden match', context => {
  html_constructor(context);
  class Disclosure extends MockHtmlElement { tagName = 'DETAILS'; open = false; }
  const outside = new Disclosure(), container = root(), outer = new Disclosure(), inner = new Disclosure(), target = new MockHtmlElement();
  container.parentElement = outside; outer.parentElement = container; inner.parentElement = outer; target.parentElement = inner;
  target.getBoundingClientRect = () => {
    assert.equal(inner.open, true, 'open the selected tool body before layout');
    assert.equal(outer.open, true, 'open nested disclosures');
    assert.equal(outside.open, false, 'do not alter disclosures outside the transcript');
    return { top: 400, bottom: 420, height: 20 } as DOMRect;
  };
  assert.equal(reveal_transcript_element(element(target)), true);
  assert.deepEqual(container.calls, [{ top: 310, behavior: 'instant' }]);
  assert.deepEqual(outside.calls, []);
  const orphan = new Disclosure(), orphan_target = new MockHtmlElement(); orphan_target.parentElement = orphan;
  assert.equal(reveal_transcript_element(element(orphan_target)), false);
  assert.equal(orphan.open, false);
});
function root(): MockHtmlElement {
  const value = new MockHtmlElement(); value.marked = true; return value;
}

test('only the nearest marked HTML root scrolls; outer ancestors and horizontal position stay unchanged', context => {
  html_constructor(context);
  const outer = root(), inner = root(), wrapper = new MockHtmlElement(), target = new MockHtmlElement();
  outer.scrollTop = 700; inner.scrollTop = 300;
  inner.top = 100; inner.bottom = 310; inner.clientTop = 5;
  wrapper.parentElement = inner; target.parentElement = wrapper; inner.parentElement = outer;
  target.top = 425; target.bottom = 465;
  assert.equal(reveal_transcript_element(element(target)), true);
  assert.deepEqual(inner.calls, [{ top: 540, behavior: 'instant' }]);
  assert.equal(inner.scrollLeft, 37);
  assert.equal(outer.scrollTop, 700);
  assert.deepEqual(outer.calls, []);
  assert.deepEqual(wrapper.calls, []);
  for (const value of [outer, inner, wrapper, target]) assert.equal(value.scroll_into_view_calls, 0);
});

test('missing target or marked ancestor performs no scroll action', context => {
  html_constructor(context);
  const outer = new MockHtmlElement(), target = new MockHtmlElement();
  target.parentElement = outer; outer.scrollTop = 91;
  assert.equal(reveal_transcript_element(null), false);
  assert.equal(reveal_transcript_element(element(target)), false);
  assert.equal(outer.scrollTop, 91);
  assert.deepEqual(outer.calls, []);
  assert.equal(target.scroll_into_view_calls, 0);
});

test('a marked non-HTML ancestor cannot replace the real HTML scroll root', context => {
  html_constructor(context);
  const container = root(), non_html = new MockElement(), target = new MockElement();
  non_html.marked = true; non_html.parentElement = container; target.parentElement = non_html;
  container.scrollTop = 10; target.top = 600; target.bottom = 620;
  assert.equal(reveal_transcript_element(element(target)), true);
  assert.deepEqual(container.calls, [{ top: 520, behavior: 'instant' }]);
  assert.equal(non_html.scroll_into_view_calls, 0);
  assert.equal(target.scroll_into_view_calls, 0);
});

test('border and current scroll state determine the content viewport center', context => {
  html_constructor(context);
  const container = root(), target = new MockElement();
  target.parentElement = container;
  container.top = 80; container.bottom = 304;
  container.clientTop = 12; container.clientHeight = 200; container.scrollTop = 150;
  target.top = 492; target.bottom = 532;
  assert.equal(reveal_transcript_element(element(target)), true);
  // Visible top is 92 and center is 192, so the content offset advances by 320.
  assert.deepEqual(container.calls, [{ top: 470, behavior: 'instant' }]);
});

test('centering remains bounded at both ends and in a container shorter than its viewport', context => {
  html_constructor(context);
  for (const [target_top, target_bottom, scroll_height, expected] of [
    [10_000, 10_020, 600, 400],
    [-1000, -900, 600, 0],
    [400, 420, 100, 0],
  ]) {
    const container = root(), target = new MockElement();
    container.scrollTop = 100; container.scrollHeight = scroll_height!;
    target.top = target_top!; target.bottom = target_bottom!; target.parentElement = container;
    assert.equal(reveal_transcript_element(element(target)), true);
    assert.deepEqual(container.calls, [{ top: expected, behavior: 'instant' }]);
    assert.ok(container.scrollTop >= 0);
    assert.ok(container.scrollTop <= Math.max(0, container.scrollHeight - container.clientHeight));
  }
});

test('an already visible target is centered within its own root without any ancestor movement', context => {
  html_constructor(context);
  const outer = root(), container = root(), target = new MockElement();
  outer.scrollTop = 500; container.parentElement = outer; target.parentElement = container;
  container.top = 10; container.clientTop = 2; container.clientHeight = 100; container.scrollTop = 200;
  target.top = 40; target.bottom = 60;
  assert.equal(reveal_transcript_element(element(target)), true);
  assert.deepEqual(container.calls, [{ top: 188, behavior: 'instant' }]);
  assert.equal(outer.scrollTop, 500);
  assert.deepEqual(outer.calls, []);
});
