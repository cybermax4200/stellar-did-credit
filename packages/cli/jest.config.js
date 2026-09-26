/** @type {import('ts-jest').JestConfigWithTsJest} */
module.exports = {
  preset: 'ts-jest',
  testEnvironment: 'node',
  transform: {
    '^.+\\.tsx?$': ['ts-jest', { diagnostics: false }],
  },
  moduleNameMapper: {
    '^@stellar-did-credit/sdk$': '<rootDir>/../sdk/src/index.ts',
  },
  testMatch: ['<rootDir>/src/**/*.test.ts'],
};
