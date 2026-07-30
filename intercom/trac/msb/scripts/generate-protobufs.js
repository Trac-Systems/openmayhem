import fs from 'fs';
import path from 'path';

import { fileURLToPath } from 'url';
import { execFileSync } from 'child_process';

function generateCJSFromProto(inputPath, outputPath) {
    execFileSync('protocol-buffers', [inputPath, '-o', outputPath]);
    console.log(`${outputPath} has been generated.`);
}

function transformToUseB4a(outputPath) {
    let content = fs.readFileSync(outputPath, 'utf-8');
    content = content.replace(/\bBuffer\.([a-zA-Z]+)/g, 'b4a.$1');
    content = `var b4a = require('b4a');\n` + content;
    fs.writeFileSync(outputPath, content, 'utf-8');
    console.log(`${outputPath} has been modified to use b4a.`);
}

function generatePbjsModule(pbjsPath, protoRootPath, entryPath, outputPath) {
    execFileSync(pbjsPath, [
        '-t', 'static-module',
        '-w', 'commonjs',
        '--keep-case',
        '-p', protoRootPath,
        '-o', outputPath,
        entryPath
    ]);
    console.log(`${outputPath} has been generated.`);
}

function transformPbjsForBare(outputPath) {
    let content = fs.readFileSync(outputPath, 'utf-8');
    const shim = `if (typeof globalThis !== 'undefined' && typeof globalThis.self === 'undefined') {\n  globalThis.self = globalThis;\n}\n`;
    const strictDirective = '"use strict";';

    if (content.includes(strictDirective)) {
        content = content.replace(strictDirective, `${strictDirective}\n${shim}`);
    } else {
        content = `${strictDirective}\n${shim}${content}`;
    }

    fs.writeFileSync(outputPath, content, 'utf-8');
    console.log(`${outputPath} has been modified for bare-compatible protobufjs runtime.`);
}

function main() {
    const directoryName = path.dirname(fileURLToPath(import.meta.url));

    const inputDir = path.join(directoryName, '../proto');
    const outputDir = path.join(directoryName, '../src/utils/protobuf');
    const pbjsPath = path.join(directoryName, '../node_modules/.bin/pbjs');
    const applyInputPath = path.join(inputDir, 'applyOperations.proto');
    const applyOutputPath = path.join(outputDir, 'applyOperations.cjs');
    const networkEntryPath = path.join(inputDir, 'network/v1/network_message.proto');
    const generatedNetworkOutputPath = path.join(outputDir, 'networkV1.generated.cjs');

    fs.mkdirSync(outputDir, { recursive: true });

    generateCJSFromProto(applyInputPath, applyOutputPath);
    transformToUseB4a(applyOutputPath);

    generatePbjsModule(pbjsPath, inputDir, networkEntryPath, generatedNetworkOutputPath);
    transformPbjsForBare(generatedNetworkOutputPath);
}

main();
