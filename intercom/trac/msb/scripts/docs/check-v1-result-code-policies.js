import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import protobufModule from '../../src/utils/protobuf/networkV1.generated.cjs';
import { ResultCode } from '../../src/utils/constants.js';
import {
    resultToValidatorAction,
    shouldEndConnection,
    SENDER_ACTION
} from '../../src/core/network/protocols/connectionPolicies.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, '..', '..');
const docPath = path.join(repoRoot, 'docs', 'v1_result_code_policies.md');
const headerLine = '| Sender | Validator | ResultCode | Value |';
const protobufResultCodeEnum = protobufModule.network?.v1?.ResultCode ?? {};

function buildGeneratedProtoResultCodes() {
    return Object.entries(protobufResultCodeEnum)
        .filter(([, value]) => typeof value === 'number')
        .map(([name, value]) => ({
            resultCodeName: name.replace(/^RESULT_CODE_/, ''),
            value
        }));
}

function parseBooleanValue(value, columnName) {
    if (value === 'true') return true;
    if (value === 'false') return false;
    throw new Error(`Unsupported ${columnName} value: ${value}. Expected true or false.`);
}

function parsePolicyTable(markdown) {
    const lines = markdown.split('\n');
    const headerIndex = lines.findIndex((line) => line.trim() === headerLine);
    if (headerIndex === -1) {
        throw new Error(`Table header not found in ${docPath}`);
    }

    const rows = [];
    for (let i = headerIndex + 2; i < lines.length; i += 1) {
        const rawLine = lines[i].trim();
        if (!rawLine.startsWith('|')) break;
        if (rawLine === '| --- | --- | --- | --- |') continue;

        const cells = rawLine
            .split('|')
            .slice(1, -1)
            .map((cell) => cell.trim());

        if (cells.length !== 4) {
            throw new Error(`Invalid table row format at line ${i + 1}: ${rawLine}`);
        }

        const [senderValue, validatorValue, resultCodeName, valueRaw] = cells;
        const value = Number(valueRaw);
        if (!Number.isInteger(value)) {
            throw new Error(`Invalid numeric value for ${resultCodeName} at line ${i + 1}: ${valueRaw}`);
        }

        rows.push({
            senderRotates: parseBooleanValue(senderValue, 'Sender'),
            validatorDisconnects: parseBooleanValue(validatorValue, 'Validator'),
            resultCodeName,
            value
        });
    }

    return rows;
}

function buildExpectedRows() {
    return Object.entries(ResultCode).map(([resultCodeName, value]) => ({
        senderRotates: resultToValidatorAction(value) === SENDER_ACTION.ROTATE,
        validatorDisconnects: shouldEndConnection(value),
        resultCodeName,
        value
    }));
}

function compareConstantsWithGeneratedProto() {
    const errors = [];
    const protoRows = buildGeneratedProtoResultCodes();
    const protoByName = new Map(protoRows.map((row) => [row.resultCodeName, row.value]));
    const constantsByName = new Map(Object.entries(ResultCode));

    for (const [resultCodeName, value] of constantsByName.entries()) {
        if (!protoByName.has(resultCodeName)) {
            errors.push(`ResultCode constant ${resultCodeName} is missing from generated protobuf enum.`);
            continue;
        }

        const protoValue = protoByName.get(resultCodeName);
        if (protoValue !== value) {
            errors.push(
                `ResultCode constant ${resultCodeName} has value ${value}, but generated protobuf enum has ${protoValue}.`
            );
        }
    }

    for (const [resultCodeName, value] of protoByName.entries()) {
        if (!constantsByName.has(resultCodeName)) {
            errors.push(
                `Generated protobuf enum exposes ${resultCodeName}=${value}, but src/utils/constants.js does not map it.`
            );
        }
    }

    return errors;
}

function compareRows(documentedRows, expectedRows) {
    const errors = [];

    if (documentedRows.length !== expectedRows.length) {
        errors.push(
            `Expected ${expectedRows.length} documented rows, found ${documentedRows.length}.`
        );
    }

    const expectedByName = new Map(expectedRows.map((row) => [row.resultCodeName, row]));
    const seenNames = new Set();

    for (const row of documentedRows) {
        if (seenNames.has(row.resultCodeName)) {
            errors.push(`Duplicate row for ${row.resultCodeName}.`);
            continue;
        }
        seenNames.add(row.resultCodeName);

        const expected = expectedByName.get(row.resultCodeName);
        if (!expected) {
            errors.push(`Unknown documented result code: ${row.resultCodeName}.`);
            continue;
        }

        if (row.value !== expected.value) {
            errors.push(
                `${row.resultCodeName} has value ${row.value} in docs, expected ${expected.value}.`
            );
        }

        if (row.senderRotates !== expected.senderRotates) {
            errors.push(
                `${row.resultCodeName} has wrong Sender policy in docs. Expected ${expected.senderRotates}.`
            );
        }

        if (row.validatorDisconnects !== expected.validatorDisconnects) {
            errors.push(
                `${row.resultCodeName} has wrong Validator policy in docs. Expected ${expected.validatorDisconnects}.`
            );
        }
    }

    for (const expected of expectedRows) {
        if (!seenNames.has(expected.resultCodeName)) {
            errors.push(`Missing documented result code: ${expected.resultCodeName}.`);
        }
    }

    for (let i = 0; i < Math.min(documentedRows.length, expectedRows.length); i += 1) {
        if (documentedRows[i].resultCodeName !== expectedRows[i].resultCodeName) {
            errors.push(
                `Unexpected row order at position ${i + 1}: expected ${expectedRows[i].resultCodeName}, found ${documentedRows[i].resultCodeName}.`
            );
        }
    }

    return errors;
}

export async function checkV1ResultCodePolicies() {
    const protobufMismatchErrors = compareConstantsWithGeneratedProto();
    if (protobufMismatchErrors.length > 0) {
        console.error('ResultCode constants are out of sync with the generated protobuf enum:');
        for (const error of protobufMismatchErrors) {
            console.error(`- ${error}`);
        }
        return false;
    }

    const markdown = await fs.readFile(docPath, 'utf8');
    const documentedRows = parsePolicyTable(markdown);
    const expectedRows = buildExpectedRows();
    const errors = compareRows(documentedRows, expectedRows);

    if (errors.length > 0) {
        console.error('V1 result code policy documentation is out of sync:');
        for (const error of errors) {
            console.error(`- ${error}`);
        }
        return false;
    }

    console.log(
        `V1 result code policy documentation is in sync (${expectedRows.length} result codes checked).`
    );
    return true;
}

const isDirectExecution = process.argv[1]
    ? import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
    : false;

if (isDirectExecution) {
    process.exitCode = (await checkV1ResultCodePolicies()) ? 0 : 1;
}
