// What addresses say about the network (src/lib/addresses.ts), in Node.
import { expect, test } from "@playwright/test";
import { addressKind, network64, onOneNetwork, pathBetween } from "../src/lib/addresses";

test("the kinds of addresses", () => {
  for (const [address, kind] of [
    ["192.168.1.20", "v4 private"],
    ["10.0.0.7", "v4 private"],
    ["172.20.10.2", "v4 private"],
    ["172.32.0.1", "v4 public"],
    ["100.72.3.4", "v4 carrier"],
    ["192.0.0.2", "v4 transition"],
    ["169.254.3.4", "v4 link-local"],
    ["127.0.0.1", "v4 loopback"],
    ["8.8.8.8", "v4 public"],
    ["2400:4050:abcd:1200::5", "v6 global"],
    ["fe80::1c2d:3e4f:5a6b:7c8d", "v6 link-local"],
    ["fd12:3456:789a::1", "v6 private"],
    ["::1", "v6 loopback"],
    ["0f6c1a52-33f1-4a8e-9a6e-0e1d2c3b4a59.local", "name"],
    ["", "hidden"],
  ] as const) {
    expect(addressKind(address), address).toBe(kind);
  }
});

test("the network part of an IPv6 address", () => {
  expect(network64("2400:4050:abcd:1200:1:2:3:4")).toBe("2400:4050:abcd:1200");
  expect(network64("2400:4050:abcd:1200::5")).toBe("2400:4050:abcd:1200");
  expect(network64("2400:4050:ABCD:0012::5")).toBe("2400:4050:abcd:12");
  expect(network64("2400::5")).toBe("2400:0:0:0");
  expect(network64("fe80::1%en0")).toBe("fe80:0:0:0");
  expect(network64("192.168.1.2")).toBeNull();
  expect(network64("2400:4050:abcd")).toBeNull();
});

test("which addresses can be on one network", () => {
  const at = (...addresses: string[]) => addresses.map((address) => ({ address }));
  // One Wi-Fi: private IPv4, and IPv6 with the same network part.
  const home = at("192.168.1.20", "2400:4050:abcd:1200:aaaa:bbbb:cccc:dddd");
  expect(onOneNetwork(at("192.168.1.31", "2400:4050:abcd:1200:1111:2222:3333:4444"), home)).toHaveLength(2);
  // A phone on mobile data: an IPv6 network of its own, and an IPv4 address that leads nowhere.
  const cellular = at("192.0.0.2", "240a:61:1234:5678:1:2:3:4");
  expect(onOneNetwork(cellular, home)).toHaveLength(0);
  expect(onOneNetwork(home, cellular).map((c) => c.address)).toEqual(["192.168.1.20"]);
  // Two phones on mobile data.
  expect(onOneNetwork(at("100.80.1.2", "240a:61:9999:1:1:2:3:4"), cellular)).toHaveLength(0);
  // A name can only be looked up from the same network.
  expect(onOneNetwork(at("0f6c1a52.local"), cellular)).toHaveLength(1);
});

test("over what a connection runs", () => {
  expect(pathBetween("192.168.1.20", "192.168.1.31")).toBe("local");
  expect(pathBetween("127.0.0.1", "127.0.0.1")).toBe("local");
  expect(pathBetween("2400:4050:abcd:1200::1", "2400:4050:abcd:1200::2")).toBe("local");
  expect(pathBetween("2400:4050:abcd:1200::1", "240a:61:1234:5678::2")).toBe("internet");
  expect(pathBetween("240a:61:1234:5678::2", "240a:61:9999:1::3")).toBe("internet");
  expect(pathBetween("fe80::1", "fe80::2")).toBe("local");
  expect(pathBetween("x.local", undefined)).toBe("unknown");
  expect(pathBetween(undefined, "2400:4050:abcd:1200::2")).toBe("unknown");
});
