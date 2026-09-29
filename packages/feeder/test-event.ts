import { xdr, Address } from "@stellar/stellar-sdk";

const val = xdr.ScVal.scvVec([
  xdr.ScVal.scvSymbol("VCAnch"),
  Address.fromString("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF").toScVal(),
  xdr.ScVal.scvBytes(Buffer.from("test-hash")),
]);

console.log(val.switch().name); // scvVec
const vec = val.vec();
if (vec) {
  console.log(vec[1].switch().name); // scvAddress
  console.log(vec[2].switch().name); // scvBytes
}
