# Security policy

## Reporting a vulnerability

Please email **hi@fatsearch.com** with a description of the issue, the steps to reproduce it and the
version or commit you tested. Do not open a public issue for a security problem.

I will acknowledge the report within a few working days, keep you informed while I work on a fix, and
credit you in the release notes unless you prefer otherwise. There is no bug bounty.

## Supported versions

Until the first stable release, only the latest commit on `main` receives security fixes.

## Scope

In scope: anything that lets input (a URL, a page, a PDF, a command line, an MCP request) read or write
outside the cache and the working directory, reach a private network address, leak a provider key, exhaust
memory or disk, or crash the binary.

Out of scope: the providers' own services (report those to the provider), and pages behind a login.
