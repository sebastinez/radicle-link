# RFC: Application Architecture

* Author: @kim
* Date: 2021-05-24
* Status: draft
* Community discussion: n/a
* Tracking issue: n/a

## Contents

-   [Motivation](#motivation)
-   [Overview](#overview)
-   [Prerequisites](#prerequisites)
    -   [Platforms](#platforms)
    -   [System Services](#system-services)
    -   [Signatures](#signatures)
    -   [Process Orchestration](#process-orchestration)
    -   [IPC](#ipc)
-   [Architecture](#architecture)
    -   [Storage](#storage)
    -   [Peer-to-peer Node](#peer-to-peer-node)
    -   [git](#git)
    -   [CLI](#cli)
    -   [Native Applications](#native-applications)
    -   [Browser Applications](#browser-applications)
-   [Future Work](#future-work)


## Motivation

Historically, development of applications on top of the libraries provided by
`radicle-link` has been driven by the [`radicle-upstream`][radicle-upstream]
application. This led to the following unpleasant situation:

* Installation of components and process management is owned by a GUI
  application, while operating on shared persistent state
* Functionality is exposed over a TCP socket, which raises some security
  concerns (see [Browser Applications](#browser-applications))
* Interoperability with other `radicle-link` applications is not defined,
  respectively tied to the GUI-based process management
* Key management is implemented as an afterthought
* It is, more generally, unclear how a monolithic architecture can support
  independent application development

Going forward, we aim to diversify application development, and thus require a
baseline architecture of how different `radicle-link` applications can interact
with each other.


## Overview

Instead of a monolithic architecture, we admit the presence of distinct
application concerns, lifecycles, and security constraints. Consequently, we lay
out a service-oriented architecture, which builds on platform-specific
primitives for process management, IPC, and credentials management. Service-
(and thus: network-) boundaries shall be defined mainly according to the
lifecycle of the application.

Note that this matters only for a usage scenario where all installed
`radicle-link` applications share the same user-scoped state. An alternative
strategy would be to scope all state to the application, ie. any two
applications would not share the same storage nor _device key_. Since this
raises the question of how those different states are synchronised in order to
provide a coherent view to the user, we haven't explored this approach further,
but admit that some clever engineering on the storage layer could enable it in
the future.

We dismiss the approach of providing a local monolithic server which encompasses
all required functionality for modularity and extensibility reasons.


## Terminology and Conventions

The key words “MUST”, “MUST NOT”, “REQUIRED”, “SHALL”, “SHALL NOT”, “SHOULD”,
“SHOULD NOT”, “RECOMMENDED”, “NOT RECOMMENDED”, “MAY”, and “OPTIONAL” in this
document are to be interpreted as described in [RFC2119] and [RFC8174] when, and
only when, they appear in all capitals, as shown here.

CBOR datatype definitions are given using the notation devised in [CDDL]. For
brevity, heterogenously typed arrays denote [array encoding][cbor-array], ie.
each field is encoded as a two-element array where the first element is a
(zero-based) index of the field in declaration order, and the second element is
the value itself (or null in the case of an absent optional value).

## Prerequisites

### Platforms

For this discussion, we divide platform targets into tiers:

* **Tier 1**:

  > Fully supported platform, receives testing, deployment, and system
  > configurations (may include packaging)

  * Linux (`x86_64-unknown-linux-gnu`)
  * macOS (`x86_64-apple-darwin`)

* **Tier 2**:

  > Basic support ("it compiles")

  * Windows
  * Other processor architectures for tier 1 platforms

* **Tier 3**:

  > Not supported, but potentially worth considering when assessing architecture
  > decisions

  * Mobile (iOS, Android)
  * Wasm


### System Services

We will rely on declarative service / process management to be provided by the
platform, specifically "socket activation".

On macOS, this is provided on macOS via [launchd]. Linux distributions
considered as tier 1 shall, however, be limited to those based on [systemd] for
the time being.

We also assume the presence of an agent capable of producing Ed25519 signatures
utilising the [SSH agent protocol][ssh-agent].

On macOS, the distribution `ssh-agent` integrates with the system keychain.
Other choices include the `ssh-agent` provided by the `openssh` suite, the
`gpg-agent`, or (on Linux systems) `gnome-keyring`.

Lastly, we consider [D-Bus][dbus] for publish-subscribe messaging. D-Bus is
available on `systemd`-based systems per default, while it requires use of an
external package manager on macOS.

> Note: a D-Bus implementation is provided on `systemd`-based platforms, and can
> be installed on macOS using `homebrew` or `MacPorts`. It may, however, turn
> out to complicate builds due to varying link-time requirements. As we only
> intend to use D-Bus for pub-sub messaging ("signals"), we may instead consider
> to provide a purpose-built in-memory pub-sub service.


### Signatures

Each process which needs to produce signatures using the `radicle-link` _device
key_, SHALL delegate this to an agent conforming to the [SSH agent protocol][ssh-agent]
(section 4.5.).

**Key access restrictions** thus depend on user configuration: the agent could
be configured to prompt for a passphrase after a timeout elapses (or never),
require presence of a hardware security token, or utilise platform
authenticators such as TouchID&trade;. Note that this presents a challenge: some
processes will reasonably require unattended access to the key (notably
[peer-to-peer nodes](#peer-to-peer-node)), while interactive use may benefit
from a user presence confirmation. This cannot be solved without restricting the
user's access to the key material, eg. by requiring a second-factor token. As we
can't assume widespread use of hardware tokens yet, we defer addressing the
issue to a future proposal.

We RECOMMEND to provide tooling for importing key material into the agent which
sets a reasonable lifetime for the key, after which it must be re-loaded from
disk (requiring a passphrase prompt for decryption).

**Key generation** SHOULD, however, be performed using `radicle-link`-supplied
tooling building on the [ed25519-zebra] library, which prevents certain weaker
keys from being generated.

**Signature verification** MUST be performed using the [ed25519-zebra] library,
and thus MUST NOT be delegated to the agent.

Note that (hypothetical) applications which require a large number of signatures
to be generated (such as data migration tools) may find `ssh-agent` throughput
insufficient, and require the raw key material to be supplied directly. This
means that long-term secure key storage remains a concern of the `radicle-link`
application suite.


### Process Orchestration

All daemon processes SHALL be started on-demand using the platform socket
activation protocol. Unless otherwise noted, daemons SHALL terminate after a
configured period of idle time (in the order of 10s of minutes). The platform
process manager MUST NOT restart socket-activated daemon processes if they exit
with a success status (except by another request for the socket).

All daemon processes SHALL run with user privileges (typically the logged-in
user). Further confinement (e.g. SELinux) is beyond the scope of this document.
It is RECOMMENDED to restrict modification of service definition files to
require super-user privileges.


### IPC

All communication with daemon processes SHALL occur over UNIX domain sockets in
`SOCK_STREAM` mode. The socket files MUST be stored in a directory only
accessible by the logged-in user (typically `$XDG_RUNTIME_DIR`), and have
permissions `0700` by default. Per-service socket paths are considered "well
known".

RPC calls over those sockets use [CBOR] for their payload encoding. As
incremental decoders are not available on all platforms, CBOR-encoded messages
shall be prepended by their length in bytes, encoded as a 32-bit unsigned
integer in network byte order.

RPC messages are wrapped in either a `request` or `response` envelope structure
as defined below:

```cddl
request = [
    request-headers,
    ? payload: bstr,
]

response = ok / error

ok = [
    response-headers,
    ? payload: bstr,
]

error = [
    response-headers,
    code: uint,
    ? message: tstr,
]

request-headers = {
    ua: client-id,
    rq: request-id,
    ? token: token,
}

response-headers = {
    rq: request-id,
}

; Unambiguous, human-readable string identifying the client application. Mainly
; for diagnostic purposes. Example: "radicle-link-cli/v1.2+deaf"
client-id: tstr .size (4..16)

; Request identifier, choosen by the client. The responder includes the
; client-supplied value in the response, enabling request pipelining.
;
; Note that streaming / multi-valued responses may include the same id in
; several response messages.
request-id: bstr .size (4..16)

; Placeholder for future one-time-token support.
token: bstr
```


## Architecture

With the prerequisites in place, we lay out the following architecture:

```
                                   +-----------------+
                                   |                 |
         +-------------------------+    Native App   +------------------------+
         |                         |                 |                        |
         |                         +--------+--------+                        |
         |                                  |                                 |
         |                      +-----------+----------+                      |
         |          +-----------|----------------------|-----------+          |
         |         /            |                      |            \         |
+--------+--------+    +--------+--------+    +--------+--------+    +--------+--------+   +----------------+
|                 |    |                 |    |                 |    |                 |   |                |
|  peer-to-peer   |    |     CLIaaS      |    |    ssh-agent    |    |      PubSub     +---+      gitd      |
|                 |    |                 |    |                 |    |                 |   |                |
+--------+--------+    +-----------------+    +--------+--------+    +--------+--------+   +-------+--------+
         |                                             |         \            |           /        |
         |                                 +-----------+          +----------------------+         |
         |                                 |                                  |           \        |
         |                         +-------+--------+                         |            +-------+--------+
         |                         |                |                         |            |                |
         +-------------------------+      CLI       +-------------------------+            |      git       |
                                   |                |                                      |                |
                                   +----------------+                                      +----------------+
```

The boxes in the middle form our "service layer", while the outer boxes
represent interactive applications. The different components are descibed in
more detail in the forthcoming sections.


### Storage

Most operations rely on accessing the storage. This interface is currently
defined in `librad`'s `git::storage::Storage`, but punts on exposing wrappers
for certain lower-level operations (components resort to `as_raw`). `librad`
exports certain higher-level operations (identities, tracking), but does not
assume exclusive ownership of the storage -- on the contrary, applications are
encouraged to utilise lower-level storage primitives directly.

For performance reasons, we do **not** at this point devise to expose storage
operations as a network service, but devise applications to link against the
`librad` and [`radicle-surf`][surf] crates directly.

The storage operations shall be revised for safe multi-process write operations,
specifically use of the [transaction][tx] API when modifying multiple refs. A
read-only variant shall be provided ([#461]). It is also suggested to extract
the base storage functionality into a separate crate, `radicle-link-storage`.


### Peer-to-peer Node

Stateful processes which interact with the `radicle-link` peer-to-peer network
come in two flavours: regular and seed nodes. Conceptually, the only difference
between them is their lifecycle: seed nodes are "always-on", whereas regular
nodes may shut down after a period of time in which no interactive operations
are conducted, in order to save system resources and for security reasons.

In practice, both node types are typically not deployed on the same kind of
machine, and specifically seed nodes may want to limit interactive use to those
needed for monitoring purposes. There is, however, no inherent reason to prevent
both node types from being deployed on the same machine. While a regular node
may be configured to behave like a seed node (e.g. an automatic tracking
configuration), we RECOMMEND to treat both as separate services, each with their
own peer id and persistent state. Whether this entails a single executable
exposing the superset of configuration options applicable to both modes of
operation, or two separate ones is an implementation choice.

The IPC socket path for peer-to-peer nodes shall follow the following
convention:

    $XDG_RUNTIME_DIR/radicle/<srv>-<peer-id>.sock

where `srv` is either "node" or "seed".

The RPC API exposed follows the interface defined by `librad`'s `net::Peer`.

> Note: this obviously excludes all `using_storage` operations, and is pending
> async-ification of the `replicate` function (which then shall be promoted to a
> `net::Peer` method).

Events emitted by the node cannot be subscribed to directly over the RPC API,
but only indirectly over the pub-sub bus. The node shall itself listen for
pub-sub messages related to publishable updates of the repository state (eg.
after a `git push`).

> TBD: describe event types, topic names and scoping (peer id)


### git

Interactions with (logical) git repositories managed by a peer-to-peer node
currently relies on a git remote helper and client-side refs rewriting. This is
mainly because of a lack of library support for rewriting refs (eg. to display
nick names alongside peer ids) on the "server". This is somewhat inconvenient,
as the remote helper needs to be placed in the user's PATH during installation,
and the (client) git configuration needs to be rewritten whenever the remote
state changes. Ideally, we should be able to use a built-in git transport to
connect to a custom git daemon, which is started on-demand via socket
activation.

Pending some advancements in the first, we can achieve this through the
`gitoxide` and `thrussh` crates, and using the `ssh` transport for "rad
remotes".

> The obvious drawback is that this requires to bind to a TCP socket, and thus
> breaks the desired isolation to the logged-in user. A workaround could be to
> supply the `ProxyCommand` option to ssh, which proxies the connection over a
> UNIX socket. While possible to achieve by modifying the user's git
> configuration, a more robust and flexible solution might be to provide a
> wrapper command (`rad push/pull`).

The git daemon emits events to the pub-sub bus after successful pushes. Like the
[post-receive] hook, this is one event per updated ref:

```cddl
updated = (
    oid-old: bstr .size 20,
    oid-new: bstr .size 20,
    namespace: bstr,
    ref: tstr        ; relative to namespace
)
```

The peer id the git daemon is assuming shall be encoded in the topic name.


### CLI

The canonical `radicle-link` CLI shall follow the "subcommand" pattern familiar
from git and other complex commandline applications. Like git, it shall be
possible to extend the set of available subcommands by placing executables
conforming to a naming convention (eg. `rad-<command>`) in the user's PATH. It
SHALL NOT be possible to override builtin commands by this mechanism.

Each subcommand MUST expose its functionality as a linkable library, and provide
CBOR serialisation for its arguments and outputs.

To enable [browser applications](#browser-applications), subcommands may be
callable over the [IPC](#ipc) protocol ("CLIaaS"). The canonical CLI may provide
a command to bind all available subcommands to a socket wholesale, or a
configurable subset of them. Unprivileged subcommands (ie. commands which do not
modify the configuration nor state) may also be exposed as a HTTP API (see
[Browser Applications](#browser-applications)).

The builtin subcommands shall include network clients for interacting with local
p2p nodes and the pub-sub bus. The vast majority of new functionality will,
however, be built on top of the [`Storage`](#storage) primitive. If and when it
becomes desirable to prototype such functionality in other programming languages
(ie. without linking to the storage library directly) it will be considered to
expose the storage interface as a network service itself.

> Note that it does not make much sense to proxy networked subcommands when
> binding to a socket. Frontends will need to connect to the respective services
> directly.

A comprehensive specification of the (initial) set of builtins is beyond the
scope of this document.


### Native Applications

Under native applications, we subsume:

1. [Electron][electron] applications
2. Applications which can not, or do not want to link against Rust library (such
   as scripted applications)

Other native applications are assumed to link against the same library modules
as the [CLI](#cli).

To facilitate rapid prototyping, but also to mitigate the risk of remote code
execution (RCE) / cross-site scripting (XSS) attacks especially for electron
applications, we RECOMMEND to develop native applications in a client-server
style, where `radicle-link` functionality is provided as [CLI](#cli) subcommands
callable over an [IPC](#ipc) socket. Recomposition of subcommands is encouraged.

For electron applications specifically, we strongly RECOMMEND to follow security
best practices. Minimally, renderer processes SHALL NOT have access to the node
environment, and proxy backend calls through the main process (renderer-main
communication utilises Chrome IPC, which is harder to attack than a TCP
connection to a backend process).


### Browser Applications

At this point, we discourage browser applications which allow modification of
the `radicle-link` state for the following reasons:

* For most practical purposes, browser-backend communication relies on a TCP
  socket. Even if bound only to the loopback interface, this poses a security
  risk due to RCE / XSS attack vectors.
* Meaningful authentication & authorization can only be achieved using
  second-factor authentication, which we don't consider feasible at this point
  (see also [Signatures](#signatures)).

Even when unprivileged, browser applications SHOULD implement an authentication
scheme using one-time / time-restricted access tokens.


## Future Work

The main omission of this RFC is to explore platform specifics for Windows
targets. This is mainly because the author has stopped using Windows two decades
ago, and was never very keen to understand the platform's idiosyncrasies. He
conjectures, however, that equivalents to the system services proposed here
exist, and would be grateful for pointers.

Another question raised during discussions which led up to this document is if
mobile platforms should be considered as first-class targets. The author's
stance on the topic is that only a subset of the `radicle-link` protocol is
applicable to resource-constrained environments, and thus a protocol revision
would be prerequisite. A traditional client-server architecture, where the
mobile device serves as a frontend to a remote service might be a first step,
however.

As alluded to throughout the document, security rests mainly on an uncompromised
user space: both malware running under the user's privileges as well as
root-level compromise can, simplified, gain access to the `SSH_AUTH_SOCK`, and
thereby compromise the application. Note that this is not specific to the
service-oriented architecture. The underlying difficulty is one of user
experience: repeated confirmation prompts tend to lead users to weaken security
by increasing intervals between prompts, or disabling them altogether. As
biometric user identification facilities become more widely deployed on consumer
hardware, we may consider encouraging their use.

Relatedly, it is left unspeficied how applications dispatch notifications which
may result in prompting the user: stateful applications may wish to present
those within their own top-level window instead of allowing daemon processes to
pop up parent-less dialogs. Intuitively, this requires a distributed locking
mechanism, which we'll leave to a future proposal.



[#461]: https://github.com/radicle-dev/radicle-link/issues/461
[AppImage]: https://appimage.org
[CBOR]: https://datatracker.ietf.org/doc/html/rfc8949
[CDDL]: https://datatracker.ietf.org/doc/html/rfc8610
[CSRF]: https://en.wikipedia.org/wiki/Cross-site_request_forgery
[RFC2119]: https://datatracker.ietf.org/doc/html/rfc2119
[RFC8032]: https://datatracker.ietf.org/doc/html/rfc8032
[RFC8174]: https://datatracker.ietf.org/doc/html/rfc8174
[cbor-array]: https://docs.rs/minicbor-derive/0.6.3/minicbor_derive/#array-encoding
[dbus]: https://freedesktop.org/Software/dbus
[ed25519-zebra]: https://github.com/ZcashFoundation/ed25519-zebra
[electron]: https://www.electronjs.org/
[launchd]: https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/Introduction.html
[post-receive]: https://git-scm.com/docs/githooks#post-receive
[radicle-upstream]: https://github.com/radicle-dev/radicle-upstream
[ssh-agent]: https://datatracker.ietf.org/doc/html/draft-miller-ssh-agent-04
[surf]: https://github.com/radicle-dev/radicle-surf
[systemd]: https://systemd.io/
[tx]: https://github.com/libgit2/libgit2/blob/main/include/git2/transaction.h
