import { checkV1ResultCodePolicies } from './check-v1-result-code-policies.js';

process.exitCode = (await checkV1ResultCodePolicies()) ? 0 : 1;
