// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title HumdRegistry — on-chain `nestling-advertise` for hum.
/// @notice Mirrors the off-chain `hum/nestlings/announce` gossip topic
///         from `ensemble::nestlings`. A humd commits the hash of its
///         NestlingManifest plus a URI pointing at the full JSON.
///         Other humds can verify "this address speaks for this
///         HumdId" without trusting hum's gossip layer.
///
/// @dev Identity model:
///        HumdId == pubkey  (the bytes32 ed25519 pubkey).
///      Whoever first calls `advertise(pubkey, ...)` claims ownership
///      of that HumdId. Subsequent updates require the same `msg.sender`.
///      Ownership transfer is intentionally NOT a feature in v1 — set
///      the right wallet before advertising.
///
///      The contract stores ONLY commitments: address(es), a hash, a
///      pointer. The manifest itself lives off-chain (IPFS, HTTPS).
///      Readers fetch the URI, hash its bytes, compare to `manifestHash`.
contract HumdRegistry {
    /// @notice One record per HumdId.
    /// @param owner address that controls this entry. Set on first advertise.
    /// @param pubkey ed25519 public key. HumdId = this value (= sha256(pubkey)
    ///        in the off-chain protocol; here we store the pubkey itself
    ///        so contracts can derive HumdId on-chain too).
    /// @param manifestHash keccak256 of the manifest JSON bytes. Readers
    ///        MUST verify the fetched payload hashes to this value.
    /// @param manifestURI pointer — `ipfs://<cid>` or `https://...`.
    /// @param updatedAt block timestamp of the latest update.
    struct Record {
        address owner;
        bytes32 pubkey;
        bytes32 manifestHash;
        string manifestURI;
        uint64 updatedAt;
    }

    /// HumdId → record.
    mapping(bytes32 => Record) public records;

    /// Emitted on every advertise (initial or update).
    /// Note: re-advertising the same manifest is allowed and re-emits
    /// the event with a fresh `updatedAt` — useful as a liveness pulse.
    event Advertised(
        bytes32 indexed humdId,
        address indexed owner,
        bytes32 manifestHash,
        string manifestURI,
        uint64 updatedAt
    );

    /// Emitted when a record's owner is rotated. v1 doesn't allow this;
    /// reserved for a future ownership-transfer extension.
    event OwnerRotated(bytes32 indexed humdId, address indexed from, address indexed to);

    /// @notice Publish or update a manifest commitment for the HumdId
    ///         derived from `pubkey`.
    /// @param pubkey the humd's ed25519 public key. HumdId in this v1
    ///        is just `pubkey` (off-chain HumdId is sha256(pubkey); a
    ///        future version can switch this if needed).
    /// @param manifestHash keccak256 of the full manifest JSON bytes.
    /// @param manifestURI fetch URI — readers MUST verify
    ///        `keccak256(bytes(payload)) == manifestHash`.
    function advertise(
        bytes32 pubkey,
        bytes32 manifestHash,
        string calldata manifestURI
    ) external {
        bytes32 humdId = pubkey;
        Record storage r = records[humdId];
        if (r.owner == address(0)) {
            r.owner = msg.sender;
            r.pubkey = pubkey;
        } else {
            require(r.owner == msg.sender, "HumdRegistry: not owner");
        }
        r.manifestHash = manifestHash;
        r.manifestURI = manifestURI;
        r.updatedAt = uint64(block.timestamp);
        emit Advertised(humdId, r.owner, manifestHash, manifestURI, r.updatedAt);
    }

    /// @notice Convenience getter — same data as the `records()` autogen
    ///         but as a memory tuple so callers can destructure cleanly.
    function get(bytes32 humdId) external view returns (Record memory) {
        return records[humdId];
    }

    /// @notice Probe whether a humdId has any record at all.
    function exists(bytes32 humdId) external view returns (bool) {
        return records[humdId].owner != address(0);
    }
}
