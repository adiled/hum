// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {HumdRegistry} from "../src/HumdRegistry.sol";
import {IHumdRegistry} from "../src/IHumdRegistry.sol";

contract HumdRegistryTest is Test {
    HumdRegistry r;

    bytes32 constant PUBKEY = bytes32(uint256(0xC0FFEE));
    bytes32 constant HASH1 = bytes32(uint256(0x1));
    bytes32 constant HASH2 = bytes32(uint256(0x2));

    function setUp() public {
        r = new HumdRegistry();
    }

    function test_advertise_first_writes_owner() public {
        r.advertise(PUBKEY, HASH1, "ipfs://abc");
        IHumdRegistry.Record memory rec = r.get(PUBKEY);
        assertEq(rec.owner, address(this));
        assertEq(rec.manifestHash, HASH1);
        assertEq(rec.manifestURI, "ipfs://abc");
        assertGt(rec.updatedAt, 0);
        assertTrue(r.exists(PUBKEY));
    }

    function test_advertise_update_keeps_owner() public {
        r.advertise(PUBKEY, HASH1, "ipfs://abc");
        uint64 firstAt = r.get(PUBKEY).updatedAt;
        vm.warp(block.timestamp + 60);
        r.advertise(PUBKEY, HASH2, "ipfs://xyz");
        IHumdRegistry.Record memory rec = r.get(PUBKEY);
        assertEq(rec.owner, address(this));
        assertEq(rec.manifestHash, HASH2);
        assertEq(rec.manifestURI, "ipfs://xyz");
        assertGt(rec.updatedAt, firstAt);
    }

    function test_advertise_other_owner_reverts() public {
        r.advertise(PUBKEY, HASH1, "ipfs://abc");
        address other = address(0xBEEF);
        vm.prank(other);
        vm.expectRevert(bytes("HumdRegistry: not owner"));
        r.advertise(PUBKEY, HASH2, "ipfs://other");
    }

    function test_event_emitted_on_advertise() public {
        vm.expectEmit(true, true, false, true);
        emit IHumdRegistry.Advertised(PUBKEY, address(this), HASH1, "ipfs://abc", uint64(block.timestamp));
        r.advertise(PUBKEY, HASH1, "ipfs://abc");
    }

    function test_exists_false_for_unknown() public view {
        assertFalse(r.exists(bytes32(uint256(0xDEADBEEF))));
    }
}
