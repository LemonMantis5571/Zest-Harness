# Getting started with Zest

Zest connects a coding conversation to a project folder. Start with one provider
and one small task; use a separate worker when you have something scoped to
delegate.

## Install and open a project

Download the package for your platform from [GitHub Releases](https://github.com/LemonMantis5571/Zest-Harness/releases).
Windows x64 installers and Linux x64 packages are available in beta. See the
[platform table](../README.md#platforms) for source-only platforms and limitations.

Launch Zest. On **Choose a provider**, use **Open** beside **Project folder
(optional)** to choose a repository. A chat without a folder is also supported;
open a project before asking Zest to inspect or change its files.

## Connect a provider

Choose the connection you already use. The options shown depend on what is
configured and available on your machine.

### Existing coding CLI sign-in

Zest can use supported coding CLIs as the main conversation provider. Install
and sign in through the CLI's own setup first, then return to Zest. For example,
the provider screen offers **Enable Claude Code** and **Enable Codex CLI** when
those connections need configuration. Availability and authentication remain
subject to that CLI and your account.

The CLI owns its credentials and session. Enabling it as your main provider does
not create a delegated worker. Follow the status and connection action shown
for the selected provider before continuing.

### Provider API key

Use the API provider form and select a preset such as **Anthropic**, **DeepSeek**,
**OpenAI**, or **OpenRouter**. Enter your own API key and complete the form. API
access and usage are governed by your provider account; an existing chat
subscription does not by itself establish API access.

Jev is off by default. To add it through OpenRouter, enter
`~typesafe/jev-latest` in **Decision model**. Zest then offers `jev_decide` to
the chat agent. Jev receives only the `state` sent in a tool call and returns
structured Choice, Score, or Noul answers with probabilities; it does not
replace the text-generating chat model.

**Check plans and changes with Jev** is a separate opt-in setting
in the OpenRouter form. Once enabled, Zest checks a completed plan before Build
and a meaningful workspace diff after a turn. It checks delegated changes after
the ordinary reviewer. Jev reports separate signals for request fit, constraints,
and completion evidence. A concern offers **Ask Zest to investigate**, which
uses the regular chat model for an explanation. Jev does not block Build or an
ordinary reviewer acceptance, and an API failure is shown as unavailable.

Plan text and bounded diff evidence go to OpenRouter only when this setting is
on. Empty, sensitive, truncated, and oversized diffs are skipped. Results are
cached by content and model so reopening a chat does not repeat the call. Zest
records delegated Jev answers and usage in `review-result.json`. For a manual
`zest.toml`, set `decision_reviewer = true` beside `decision_model` under the
OpenRouter provider. Only one provider can be the Jev reviewer.

The concern threshold starts at 0.90 and is provisional. Before enabling this
for a broad release, evaluate labeled plan gaps and change mismatches, and
record false alerts, missed issues, response time, and OpenRouter cost from
the stored usage. Jev's probabilities are signals, not proof of plan or code
quality.

To use it, ask the chat agent to make a bounded decision, such as: "Use Jev to
classify this issue as billing, account, or technical, and include the
probabilities." The chat model builds the choices and supplies the issue text
as `state`.

Keys are not written into `zest.toml`. Zest uses the OS credential manager when
available. Do not paste a key into chat or put one in a screenshot or bug report.

### Compatible or local endpoint

Choose **Custom** in the API provider form and enter:

| Field | What to enter |
| --- | --- |
| Provider id | A short name you will recognize, such as `local-llm` |
| Base URL | The OpenAI-compatible API URL supplied by your server |
| Default model | The exact model ID exposed by that endpoint |
| Allowed models | The model IDs you want to make available |
| API key | Credentials required by your endpoint |

Start the local server first if you use one. Compatibility depends on the
server's API and model capabilities; a model that supports text generation may
not support the tools needed to edit a repository.

## Complete a small task

After connecting, continue into chat and send:

> Explain this repository and suggest one small improvement. Do not change files yet.

Check that the answer refers to your project. Choose one suggestion and follow
up with a specific request, for example:

> Make that change, explain the diff, and run the relevant existing tests.

Respond to any approval requests as they appear. The approval mode beside the
composer controls when Zest asks; approved commands still run with your OS
permissions.

When a change is available, open the branch changes bar above the composer to
inspect the diff. Check the test result in the conversation before deciding
whether to keep the change. Tests run only when requested or invoked by the
agent; a completed response is not proof that tests passed.

Use the model control in the composer to select a model and, where supported,
its reasoning effort. Model availability comes from the connected provider.

## When to delegate

A feature card describes a separate task: objective, scope, selected context,
acceptance checks, worker and reviewer. Use it for work with a clear result you
can review. Delegation is opt-in, and your main conversation stays with its
selected provider. Read [Delegation](../README.md#delegate-a-scoped-task) and
[the coordinator guide](SERVE.md) for the execution and review workflow.

## Ask a side question

Type `/btw Why did you choose this approach?` in the main composer to open a
temporary side conversation. `/btw` on its own opens an empty question box.
Follow-up questions stay in that panel. Close it or press **Esc** to return to
the main chat without adding the side exchange to its context.

You can also ask while the main task is working; the side conversation starts
from its last completed turn. Its **Stop** button cancels only the side answer.
In the terminal client, use `/btw [question]` and `/back` to return.

Side questions use the selected provider and count toward usage. Cache reuse
depends on the provider; `/btw` does not guarantee free input tokens. See
[side conversations](BTW.md) for provider behavior and storage details.

## Setup help

| What you see | Next step |
| --- | --- |
| A CLI is unavailable | Install and sign in to the supported CLI, then refresh the provider list. |
| Connection check failed / Reconnect | Use the selected provider's reconnect action and check its sign-in or API credentials. |
| This folder cannot be used as a project | Use **Choose a different folder** next to the error. |
| No project selected / No workspace | Open the repository folder before asking about its files. |
| A custom model is unavailable | Check the endpoint is running and the default/allowed model IDs match the server. |

Config lives at `~/.zest`; selected context and requests go to your chosen
provider. See [provider quota](QUOTA.md), [support](../SUPPORT.md), and
[contributing](../CONTRIBUTING.md) for usage information and developer setup.
