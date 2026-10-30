# Security policy

sbt-server is a local daemon (`sbtd`) that runs sbt on your behalf, and sbt
runs arbitrary code through a build definition. A request that reaches
`sbtd` from anywhere other than a process on the same machine is therefore
code execution, which is why reports about the daemon's reachability are
taken seriously.

## Reporting a vulnerability

Please do not open a public issue for a security problem.

Report it privately through GitHub's vulnerability reporting for this
repository: https://github.com/scottweaver/sbt-server/security/advisories/new
("Report a vulnerability" under the Security tab). The report is visible
only to the maintainer until a fix is published.

Include what you can of:

- the version (`sbtctl --version`) and platform;
- the exact request, file or command that triggers the problem;
- what an attacker gains (code execution, reading build output, a denial of
  service of the daemon);
- a proof of concept if you have one.

## What to expect

- Acknowledgement within 3 days of the report.
- A fix, or a decision that the report is out of scope with the reasoning,
  within 30 days. Reports that reach `/exec` from a web page or another
  host get a fix as fast as one can be released.
- Coordinated disclosure: the advisory is published together with the fixed
  release, and you are credited unless you ask not to be.

## Supported versions

Only the latest release receives security fixes. Upgrade with
`brew upgrade sbt-server` or the installer attached to the latest release.

## Scope

In scope:

- any way for a request that did not originate on the local machine to
  reach `sbtd` (DNS rebinding, a browser, another host), or to make `sbtd`
  connect to a server it should not;
- `sbtd` trusting a file it should validate (`active.json`, a token file);
- secrets or raw build output written where other users can read them.

Out of scope, by design (see `.claude/rules/ARCHITECTURE.md`):

- `sbtd` has no authentication beyond binding to loopback and checking the
  `Host` and `Origin` headers; a local process that can reach loopback can
  run sbt, exactly as it could run `sbt` itself. Adding bearer-token auth is
  tracked separately.
- Anything that requires an attacker to already run code as your user.
