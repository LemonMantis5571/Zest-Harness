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
or **OpenAI**. Enter your own API key and complete the form. API access and usage
are governed by your provider account; an existing chat subscription does not
by itself establish API access.

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
