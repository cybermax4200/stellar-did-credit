declare module "snarkjs" {
  const snarkjs: {
    groth16: {
      fullProve(
        input: Record<string, unknown>,
        circuitWasm: string | URL,
        verificationKey: string | URL,
      ): Promise<{ proof: unknown; publicSignals: string[] }>;
      verify(
        verificationKey: unknown,
        publicSignals: string[],
        proof: unknown,
      ): Promise<boolean>;
    };
    zKey: {
      exportVerificationKey(zkeyPath: string | URL): Promise<unknown>;
    };
  };
  export default snarkjs;
  export = snarkjs;
}
