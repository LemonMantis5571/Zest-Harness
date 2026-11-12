// Upstream anti-slop tests for the vendored rules (commit c44ef22ca116),
// unchanged except for imports. Run through `npm run ui:lint:plugins`.
import { RuleTester } from "oxlint/plugins-dev";

import plugin from "./anti-slop.mjs";

const tester = new RuleTester({ languageOptions: { parserOptions: { lang: "ts" } } });
const rule = (name) => plugin.rules[name];

tester.run("anti-slop/no-array-filter-map", rule("no-array-filter-map"), {
  valid: [
    "const users = []; users.values().filter(active).map(email).toArray();",
    "const users = []; users.values().map(email).filter(Boolean).toArray();",
    "Iterator.from(users).filter(active).map(email).toArray();",
    "function collect(users: IteratorObject<User>) { return users.filter(active).map(email).toArray(); }",
    "const users = []; users.flatMap(user => user.active ? [user.email] : []);",
    "const users = []; users.map(email); users.filter(active);",
    "const users = []; users.map(email).map(normalize);",
    "const users = []; users.filter(active).filter(verified);",
    "const custom = { filter() { return this; }, map() {} }; custom.filter(active).map(email);",
    "function collect(unknownReceiver) { return unknownReceiver.filter(active).map(email); }",
    "const users = fetchUsers(); users.filter(active).map(email);",
    "const users = []; function collect(users) { return users.filter(active).map(email); }",
    "let users = []; users = iterator; users.filter(active).map(email);",
    "const users = []; users[method](active).map(email);",
    "const first = second; const second = first; first.filter(active).map(email);",
  ],
  invalid: [
    { code: "[].filter(active).map(email);", errors: [{ messageId: "arrayFilterMap" }] },
    {
      code: "[].map(user => user.active ? user.email : undefined).filter(email => email !== undefined);",
      errors: [{ messageId: "arrayFilterMap" }],
    },
    { code: "const users = []; users.map(email).filter(Boolean);", errors: [{ messageId: "arrayFilterMap" }] },
    {
      code: "const users = []; const alias = users; alias.filter(active).map(email);",
      errors: [{ messageId: "arrayFilterMap" }],
    },
    {
      code: "function collect(users: User[]) { return users.filter(active).map(email); }",
      errors: [{ messageId: "arrayFilterMap" }],
    },
    {
      code: "function collect(users: readonly User[]) { return users.map(email).filter(present); }",
      errors: [{ messageId: "arrayFilterMap" }],
    },
    {
      code: "function collect(users: ReadonlyArray<User>) { return users.filter(active).map(email); }",
      errors: [{ messageId: "arrayFilterMap" }],
    },
    {
      code: "function collect(users: Array<User>) { return users.filter(active).map(email); }",
      errors: [{ messageId: "arrayFilterMap" }],
    },
    {
      code: "const users = [] as const; users['filter'](active)['map'](email);",
      errors: [{ messageId: "arrayFilterMap" }],
    },
    { code: "const users = []; (users.filter(active)!).map(email);", errors: [{ messageId: "arrayFilterMap" }] },
    { code: "const users = []; users?.filter(active)?.map(email);", errors: [{ messageId: "arrayFilterMap" }] },
    { code: "const users = []; users.slice().filter(active).map(email);", errors: [{ messageId: "arrayFilterMap" }] },
    {
      code: "const users = []; users.filter(active).map(email).filter(Boolean);",
      errors: [{ messageId: "arrayFilterMap" }, { messageId: "arrayFilterMap" }],
    },
  ],
});

tester.run("anti-slop/no-reduce-accumulator-copy", rule("no-reduce-accumulator-copy"), {
  valid: [
    "items.reduce((acc, item) => { acc.push(item); return acc; }, []);",
    "items.reduce((acc, item) => Object.assign(acc, item), {});",
    "items.reduce((acc, item) => Object.assign(acc, acc, item), {});",
    "items.reduce((acc, item) => { acc[item.id] = { ...item }; return acc; }, {});",
    "items.reduce((acc, item) => { acc.push(Object.assign({}, item)); return acc; }, []);",
    "items.reduce((acc, item) => { acc.push(item.slice()); return acc; }, []);",
    "items.reduce((acc, item) => acc.concat(item), '');",
    "items.reduce((acc, item) => acc.concat(item), customCollection);",
    "function copy(acc) { return Object.assign({}, acc); }",
    "items.map((acc, item) => Object.assign({}, acc));",
    "items.reduce((acc, item) => { function copy(acc) { return Object.assign({}, acc); } return acc; }, {});",
    "items.reduce((acc, item) => { const snapshot = () => Object.assign({}, acc); return acc; }, {});",
    "items.reduce((acc, item) => { { const acc = {}; Object.assign({}, acc); } return acc; }, {});",
    "const Object = custom; items.reduce((acc, item) => Object.assign({}, acc), {});",
    "function run(Object) { return items.reduce((acc, item) => Object.assign({}, acc), {}); }",
    "const Array = custom; items.reduce((acc, item) => Array.from(acc), []);",
    "items.reduce((acc, item) => { let alias = acc; alias = item; return Object.assign({}, alias); }, {});",
    "items.reduce((acc, item) => [...acc, item], []);",
    "items.reduce((acc, item) => ({ ...acc, [item.id]: item }), {});",
  ],
  invalid: [
    "items.reduce((acc, item) => Object.assign({}, acc, { [item.id]: item }), {});",
    "items.reduceRight((acc, item) => Object.assign({}, acc, item), {});",
    "items.reduce((acc, item, index, array) => Object.assign({}, acc, item), {});",
    "items.reduce(acc => Object.assign({}, acc), {});",
    "items.reduce(function (acc, item) { return Object.assign({}, item, acc); }, {});",
    "items['reduce'](((acc, item) => Object['assign']({}, acc, item)), {});",
    "items.reduce((acc = {}, item) => Object.assign({}, acc, item), {});",
    "items.reduce((acc, item) => { const alias = acc; return Object.assign({}, alias, item); }, {});",
    "items.reduce((acc, item) => Object.assign({}, acc as State, item), {});",
    "items.reduce((acc, item) => { const next = Object.assign({}, acc); next[item.id] = item; return next; }, {});",
    "items.reduce((acc, item) => acc.concat([item]), []);",
    "items.reduceRight((acc, item, index) => acc['concat']([item]), [] as Item[]);",
    "items.reduce((acc, item) => { const next = acc.slice(); next.push(item); return next; }, []);",
    "items.reduce((acc, item) => { const alias = acc; return alias.concat(item); }, []);",
    "const initial = []; items.reduce((acc, item) => acc.concat(item), initial);",
    "items.reduce((acc, item) => { const next = Array.from(acc); next.push(item); return next; }, []);",
    "items.reduce((acc, item) => acc.toSpliced(acc.length, 0, item), []);",
    "items.reduce((acc, item) => acc.toSorted(), []);",
    "items.reduce((acc, item) => acc.toReversed(), []);",
    "items.reduce((acc, item) => acc.with(0, item), []);",
  ].map((code) => ({ code, errors: [{ messageId: "accumulatorCopy" }] })),
});

tester.run("anti-slop/no-chained-type-assertions", rule("no-chained-type-assertions"), {
  valid: [
    "const value = input as User;",
    "const value = (input as User);",
    "const value = ({ id: 1 } as const) as const;",
  ],
  invalid: [
    "const value = input as unknown as User;",
    "const value = (input as unknown) as User;",
    "const value = <User>(<unknown>input);",
    "const value = ({ id: 1 } as const) as User;",
  ].map((code) => ({ code, errors: [{ messageId: "chained" }] })),
});

if (rule("no-conditional-empty-object-spread").meta?.fixable !== undefined) {
  throw new Error("no-conditional-empty-object-spread must not offer a semantics-changing fix.");
}
tester.run("anti-slop/no-conditional-empty-object-spread", rule("no-conditional-empty-object-spread"), {
  valid: [
    "const result = { value };",
    "const result = { ...values };",
    "const result = condition ? { value } : {};",
  ],
  invalid: [
    "const result = { ...(value !== undefined ? { value } : {}) };",
    "const result = { ...(condition ? {} : { value }) };",
  ].map((code) => ({ code, errors: [{ messageId: "avoid" }] })),
});

tester.run("anti-slop/no-module-mocking", rule("no-module-mocking"), {
  valid: [
    "const store = new InMemoryUserStore();",
    "vi.spyOn(store, 'save');",
    "const vi = { mock() {} }; vi.mock();",
    "function test(jest: { mock(): void }) { jest.mock(); }",
    "import { vi as localVi } from './helpers'; localVi.mock('./module');",
  ],
  invalid: [
    "vi.mock('./user-store');",
    "jest.mock('./user-store');",
    "vi['doMock']('./user-store');",
    "jest.unstable_mockModule('./user-store');",
    "import { vi } from 'vitest'; vi.mock('./user-store');",
    "import { vi as testApi } from 'vitest'; testApi.mock('./user-store');",
    "import { jest } from '@jest/globals'; jest.mock('./user-store');",
  ].map((code) => ({ code, errors: [{ messageId: "moduleMock" }] })),
});

tester.run("anti-slop/no-widen-then-assert", rule("no-widen-then-assert"), {
  valid: [
    "const source = { id: 'first' }; const widened: unknown = source;",
    "declare const input: unknown; const parsed = input as { readonly id: string };",
  ],
  invalid: [
    {
      code: "const source = { id: 'second' }; const widened: unknown = source; const parsed = widened as { readonly id: string };",
      errors: [{ messageId: "widenThenAssert" }],
    },
  ],
});
