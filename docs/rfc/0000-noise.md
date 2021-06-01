
# RFC: Noise Protocol

* Author: @kim
* Date: 2021-05-31
* Status: on hodl
* Community discussion: n/a
* Tracking issue: n/a

## Motivation

The initial design document for the [Radicle Link Protocol][link-draft0]
suggested utilising the [Noise Protocol Framework][noise] for transport-layer
security. However, lack of standardisation and availability of implementations
for [QUIC] prevented this from getting adopted, and instead an authentication
and encryption scheme based on self-signed [TLS 1.3][tls] certificates was
implemented.

[Noise][noise] is preferrable over [TLS][tls] in the context of peer-to-peer
networking, because it is considerably simpler and more compact.  Noise also
doesn't mandate implementation of a PKI, which, in an open peer-to-peer network,
is not applicable. Deliberately defeating the PKI mechanisms of TLS for our
purposes, however, yields dubious security properties, and may be a source of
bugs in implementations.

Recently, the [ipfs-embed] project has released an experimental implementation
of [Noise transport security for QUIC][quinn-noise], and registered a [QUIC
version range][QUIC-versions]. Due to the similarities in purpose and
implementation, this implementation would be usable as-is for `radicle-link`.


## Overview

When considering a migration, the following questions arise:

* Can the existing `radicle-link` network be migrated in a backwards-compatible
  way?
* What protocol-level changes are required, if any?
* Is it acceptable to use an experimental implementation of a cryptographic
  protocol, and how do we assess it?

We lay out those questions in more detail in [Discussion](#discussion),
before devising a [Recommendation](#recommendation).

## Discussion

### Migration

Unfortunately, [QUIC] does not specify a version negotiation protocol which
would allow two parties to agree on a mutually supported version -- for fear
downgrade attacks which have plagued TLS for years, the only measure it takes is
to devise servers to reject connection attempts if no matching version from an
unordered list offered by the client is found. This [may change][quic-version-negotiation],
but for now precludes using the QUIC version as an upgrade path: due to the lack
of a more detailed specification, [quinn] has decided to not make the protocol
version chosen by the server available to the connecting client.

This leaves little choice:

1. Implement the session interface such that it inspects the first few bytes and
   either delegates to the TLS or Noise handshake
2. Advertise both the regular and Noise-reserved QUIC protocol versions when
   accepting connections
3. Do the same when initiating connections, but handle the case of a version
   mismatch error and attempt a new connection with only TLS
4. Measure success rates on a handful of known seed nodes, and stop offering TLS
   at a convenient time


### Protocol Changes

We currently use [ALPN] to advertise protocol versions as well as logical
networks. Exchange of peer information (including capabilities) is deferred to
when and if a peer actually participates in gossip (ie. peer information is not
exchanged for any other stream types).

[quinn-noise] chooses the obvious Noise handshake pattern for peer-to-peer
networks: `IKpsk1`, which requires the initiator to know the peer's static key
upfront, and allows using a PSK to create private or restricted networks.

This would mean that a version negotiation packet would need to be exchanged
right after the handshake, before any stream upgrades. It is tempting to
conflate this with the peer info exchange, at a tolerable loss of leniency. An
alternative solution would be to reserve a QUIC version range, although that may
limit the total number of possible incompatible `radicle-link` protocol
upgrades.

At the cost of additional RTT, the application-level post-handshake exchange
could be made compatible in a mixed-crypto deployment. It is not obvious how
that could be retained using a QUIC version.


### Experimental Cryptography

`quinn-noise` rely on the (fairly established) [`snow`][snow] implementation of
Noise, but on an implementation of the [Xoodyak][xoodyak] construction
specifically written for the purpose ([`xoodoo`][xoodoo]). Unlike the
[`rustls`][rustls] library underpinning our TLS stack, neither `xoodoo` nor
`quinn-noise` have received a formal security audit yet. To be fair though, the
same is true for `snow`.

`radicle-link` currently does not make any strong assumptions about the
transport-layer security, and is still in an experimental stage. It is thus
debatable if auditing is a prerequisite for adopting the implementation. On the
other hand, `ipfs-embed` is developed in the context of the larger IPFS
ecosystem, and itself in a very early stage. It is likely that the
implementation will receive more scrutiny as it matures.


### Key Reuse

For completeness, we point out that Noise explicitly [discourages][noise-sec]
re-use of static keys for other purposes than the Noise handshake.
`radicle-link` does deliberately re-use the certificate key to sign identity
documents and refs advertisements: those uses are considered equivalent to a PKI
in which the leaf certificate is to be signed by the device-local key.

As long as the principles of it's [TUF] heritage are adhered to -- namely that
key _delegation_ should be used for extension functionality -- we do not see a
reason for concerns.


# Recommendation

While it is tempting to either adopt `quinn-noise` or a custom implementation as
quickly as possible while the `radicle-link` network and protocol are not
considered stable, it is also not _harmful_ to keep using TLS for the time
being. The recommendation is thus to revisit the state of affairs when
`ipfs-embed` reaches a stage where it is used in other projects, and keep this
proposal on hold until then.


[ALPN]: https://datatracker.ietf.org/doc/html/rfc7301
[QUIC-versions]: https://github.com/quicwg/base-drafts/wiki/QUIC-Versions
[QUIC]: https://datatracker.ietf.org/doc/html/rfc9000
[TLS]: https://datatracker.ietf.org/doc/html/rfc8446
[TUF]: https://theupdateframework.io/
[ipfs-embed]: https://github.com/ipfs-rust/ipfs-embed
[link-draft0]: ../spec/drafts/radicle-link-rev1-draft.md
[noise-sec]: https://noiseprotocol.org/noise.html#security-considerations
[noise]: https://noiseprotocol.org/noise.html
[quic-version-negotiation]: https://datatracker.ietf.org/doc/html/draft-ietf-quic-version-negotiation
[quinn-noise]: https://github.com/ipfs-rust/quinn-noise
[quinn]: https://github.com/quinn-rs/quinn
[rustls]: https://crates.io/crates/rustls
[snow]: https://crates.io/crates/snow
[xoodoo]: https://github.com/ipfs-rust/xoodoo
[xoodyak]: https://csrc.nist.gov/CSRC/media/Projects/lightweight-cryptography/documents/round-2/spec-doc-rnd2/Xoodyak-spec-round2.pdf
