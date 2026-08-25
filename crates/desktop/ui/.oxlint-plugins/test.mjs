import { RuleTester } from "oxlint/plugins-dev";

import plugin from "./zest-boundaries.mjs";

const tsTester = new RuleTester({
  languageOptions: { parserOptions: { lang: "ts" } },
});
const tsxTester = new RuleTester({
  languageOptions: { parserOptions: { lang: "tsx" } },
});

tsTester.run("no-unvalidated-persisted-json", plugin.rules["no-unvalidated-persisted-json"], {
  valid: [
    "const value: unknown = JSON.parse(raw);",
    "const value: Record<string, unknown> = JSON.parse(raw);",
    "const value = JSON.parse(raw);",
  ],
  invalid: [
    { code: "type User = { id: string }; const value = JSON.parse(raw) as User;", errors: 1 },
    { code: "type User = { id: string }; const value: User = await response.json();", errors: 1 },
    { code: "type User = { id: string }; const raw = JSON.parse(input); const value = raw as User;", errors: 1 },
    { code: "type User = { id: string }; const value = response.json<User>();", errors: 1 },
  ],
});

tsTester.run("no-secret-persistence-or-sink", plugin.rules["no-secret-persistence-or-sink"], {
  valid: [
    "localStorage.setItem('prefs', JSON.stringify({ theme }));",
    "console.info('provider ready');",
    "console.debug('tokens used', tokenCount);",
  ],
  invalid: [
    { code: "sessionStorage.setItem('session', accessToken);", errors: 1 },
    { code: "console.error('auth failed', clientSecret);", errors: 1 },
  ],
});

tsxTester.run("require-safe-html-provenance", plugin.rules["require-safe-html-provenance"], {
  valid: [
    "const node = <div dangerouslySetInnerHTML={{ __html: markTrustedHtml(html, 'shiki') }} />;",
  ],
  invalid: [
    { code: "const node = <div dangerouslySetInnerHTML={{ __html: html }} />;", errors: 1 },
  ],
});

tsTester.run("no-unowned-background-rejection", plugin.rules["no-unowned-background-rejection"], {
  valid: [
    "promise.catch((error) => reportBackgroundFailure(error));",
    "promise.catch((error) => { if (error) reportBackgroundFailure(error); });",
  ],
  invalid: [
    { code: "promise.catch(() => {});", errors: 1 },
    { code: "promise.catch(() => undefined);", errors: 1 },
  ],
});

tsTester.run("no-object-url-leak", plugin.rules["no-object-url-leak"], {
  valid: [
    "const url = URL.createObjectURL(blob); URL.revokeObjectURL(url);",
    "const url = window.URL.createObjectURL(blob); window.URL.revokeObjectURL(url);",
    "const url = URL.createObjectURL(blob); const alias = url; URL.revokeObjectURL(alias);",
  ],
  invalid: [
    { code: "const url = URL.createObjectURL(blob); use(url);", errors: 1 },
    { code: "let url = URL.createObjectURL(first); url = URL.createObjectURL(second); URL.revokeObjectURL(url);", errors: 1 },
  ],
});

console.log("Zest Oxlint plugin tests passed.");
