// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IHumdRegistry} from "./IHumdRegistry.sol";

/// @title HumdRegistry — the vanilla [`IHumdRegistry`] implementation.
/// @notice Open advertise: anyone can claim a `pubkey` slot by being
///         the first to call `advertise()` with it. Subsequent updates
///         require the same `msg.sender`. No fees, no allowlist, no
///         signature verification — those are layered in other
///         implementations of the same interface.
///
/// @dev Deploy your own per subnet. There is no canonical address —
///      see `contracts/DEPLOYMENTS.md`.
contract HumdRegistry is IHumdRegistry {
    /// @notice HumdId → record. Public mapping autogen matches the
    ///         `records(bytes32)` getter in [`IHumdRegistry`].
    mapping(bytes32 => Record) internal _records;

    function advertise(
        bytes32 pubkey,
        bytes32 manifestHash,
        string calldata manifestURI
    ) external override {
        bytes32 humdId = pubkey;
        Record storage r = _records[humdId];
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

    function get(bytes32 humdId) external view override returns (Record memory) {
        return _records[humdId];
    }

    function exists(bytes32 humdId) external view override returns (bool) {
        return _records[humdId].owner != address(0);
    }

    function records(bytes32 humdId) external view override returns (
        address owner,
        bytes32 pubkey,
        bytes32 manifestHash,
        string memory manifestURI,
        uint64 updatedAt
    ) {
        Record storage r = _records[humdId];
        return (r.owner, r.pubkey, r.manifestHash, r.manifestURI, r.updatedAt);
    }
}
