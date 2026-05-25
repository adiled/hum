// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title IHumdRegistry — the wire-protocol of the hum on-chain identity layer.
/// @notice **This interface is the standard.** Anyone can deploy a
///         contract implementing it — vanilla, allowlisted, stake-backed,
///         name-resolving, whatever — and humds + bees can read it
///         the same way. There is no canonical hum-maintainer-deployed
///         registry today, and the protocol does not require one.
///
/// @dev The shape `(advertise, get, exists, records, Advertised)` is
///      what off-chain clients (the Rust `ensemble::onchain` module,
///      future TS/Python/Go clients) decode against. Any contract that
///      implements this interface is a valid HumdRegistry from the
///      mesh's perspective.
///
///      Why an interface, not just a contract:
///        - **Subnets**. A hackathon team, a single org, or one
///          autonomous agent can deploy any implementation they like;
///          all clients keep working.
///        - **Plug-in policy**. Allowlists, stake-on-advertise, name
///          aliases — all live in the implementation, not in the
///          off-chain wire.
///        - **Future migration**. If a v2 interface ships, v1 stays
///          callable by older humds during the transition.
interface IHumdRegistry {
    /// @notice One record per HumdId.
    /// @param owner        address that controls this entry
    /// @param pubkey       ed25519 public key (HumdId == this value in v1)
    /// @param manifestHash keccak256 of the manifest JSON bytes
    /// @param manifestURI  fetch URI — `ipfs://<cid>` or `https://...`
    /// @param updatedAt    block timestamp of the latest update
    struct Record {
        address owner;
        bytes32 pubkey;
        bytes32 manifestHash;
        string manifestURI;
        uint64 updatedAt;
    }

    /// @notice Emitted on every advertise (initial or re-advertise pulse).
    event Advertised(
        bytes32 indexed humdId,
        address indexed owner,
        bytes32 manifestHash,
        string manifestURI,
        uint64 updatedAt
    );

    /// @notice Publish or update a manifest commitment for `pubkey`.
    /// @dev Implementations decide whether updates are open, owner-locked,
    ///      allowlisted, or stake-gated. Vanilla implementation is
    ///      owner-locked: first caller for a given `pubkey` claims it.
    function advertise(
        bytes32 pubkey,
        bytes32 manifestHash,
        string calldata manifestURI
    ) external;

    /// @notice Memory-tuple getter — symmetric with the autogen `records`
    ///         public mapping. Implementations MUST return the same data.
    function get(bytes32 humdId) external view returns (Record memory);

    /// @notice Probe whether a `humdId` has any record at all.
    function exists(bytes32 humdId) external view returns (bool);

    /// @notice Autogen public-mapping getter signature. Implementations
    ///         may or may not expose a literal `records` mapping; the
    ///         off-chain Rust client uses this selector
    ///         (`0x52e84c43`) for cheap reads. Equivalent to `get`.
    function records(bytes32 humdId) external view returns (
        address owner,
        bytes32 pubkey,
        bytes32 manifestHash,
        string memory manifestURI,
        uint64 updatedAt
    );
}
