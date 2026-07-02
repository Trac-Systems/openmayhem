import fs from 'fs';
import path from 'path';

import { fileURLToPath } from 'url';
import { execSync } from 'child_process';

function generateCJSFromProto(inputPath, outputPath) {
    execSync(`protocol-buffers "${inputPath}" -o "${outputPath}"`);
    console.log(`${outputPath} has been generated.`);
}

function transformToUseB4a(outputPath) {
    let content = fs.readFileSync(outputPath, 'utf-8');
    content = content.replace(/\bBuffer\.([a-zA-Z]+)/g, 'b4a.$1');
    content = `var b4a = require('b4a');\n` + content;
    fs.writeFileSync(outputPath, content, 'utf-8');
    console.log(`${outputPath} has been modified to use b4a.`);
}


function main() {
    const directoryName = path.dirname(fileURLToPath(import.meta.url));

    const inputDir = path.join(directoryName, '../proto');
    const outputDir = path.join(directoryName, '../src/utils/protobuf');

    fs.mkdirSync(outputDir, { recursive: true });

    const files = fs.readdirSync(inputDir).filter(f => f.endsWith('.proto'));

    for (const file of files) {
        const name = path.basename(file, '.proto');
        const inputPath = path.join(inputDir, file);
        const outputPath = path.join(outputDir, `${name}.cjs`);

        generateCJSFromProto(inputPath, outputPath);
        transformToUseB4a(outputPath);
    }
}

main();

