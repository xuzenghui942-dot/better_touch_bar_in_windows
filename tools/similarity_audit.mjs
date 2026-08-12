import fs from "node:fs";
import path from "node:path";

const [candidateRoot, referenceRoot, requestedShingleSize] =
  process.argv.slice(2);
if (!candidateRoot || !referenceRoot) {
  console.error(
    "Usage: node tools/similarity_audit.mjs <candidate-root> <reference-root> [shingle-tokens]"
  );
  process.exit(2);
}

const SHINGLE_SIZE = requestedShingleSize
  ? Number.parseInt(requestedShingleSize, 10)
  : 32;
if (!Number.isInteger(SHINGLE_SIZE) || SHINGLE_SIZE < 8) {
  console.error("shingle-tokens must be an integer of at least 8");
  process.exit(2);
}

function rustFiles(root) {
  const files = [];
  const visit = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      if (["target", ".git", "node_modules"].includes(entry.name)) {
        continue;
      }
      const fullPath = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        visit(fullPath);
      } else if (entry.isFile() && entry.name.endsWith(".rs")) {
        files.push(fullPath);
      }
    }
  };
  visit(path.resolve(root));
  return files.sort();
}

function tokens(file) {
  const source = fs
    .readFileSync(file, "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/\/\/.*$/gm, " ");
  return (
    source.match(
      /[A-Za-z_][A-Za-z0-9_]*|0x[0-9A-Fa-f]+|\d+(?:\.\d+)?|[^\s]/g
    ) ?? []
  );
}

function shingles(file) {
  const values = tokens(file);
  const result = new Set();
  for (let index = 0; index + SHINGLE_SIZE <= values.length; index += 1) {
    result.add(values.slice(index, index + SHINGLE_SIZE).join(" "));
  }
  return result;
}

function intersectionSize(left, right) {
  const [small, large] =
    left.size <= right.size ? [left, right] : [right, left];
  let count = 0;
  for (const value of small) {
    if (large.has(value)) {
      count += 1;
    }
  }
  return count;
}

const candidateFiles = rustFiles(candidateRoot);
const referenceFiles = rustFiles(referenceRoot);
const candidateSets = new Map(
  candidateFiles.map((file) => [file, shingles(file)])
);
const referenceSets = new Map(
  referenceFiles.map((file) => [file, shingles(file)])
);

const allCandidate = new Set(
  [...candidateSets.values()].flatMap((values) => [...values])
);
const allReference = new Set(
  [...referenceSets.values()].flatMap((values) => [...values])
);
const shared = intersectionSize(allCandidate, allReference);

let closestPair = null;
for (const [candidate, candidateShingles] of candidateSets) {
  for (const [reference, referenceShingles] of referenceSets) {
    const pairShared = intersectionSize(candidateShingles, referenceShingles);
    if (pairShared === 0) {
      continue;
    }
    const containment =
      pairShared / Math.max(1, Math.min(candidateShingles.size, referenceShingles.size));
    if (!closestPair || containment > closestPair.containment) {
      closestPair = {
        candidate: path.relative(candidateRoot, candidate),
        reference: path.relative(referenceRoot, reference),
        shared: pairShared,
        containment,
      };
    }
  }
}

console.log(
  JSON.stringify(
    {
      shingleTokens: SHINGLE_SIZE,
      candidateFiles: candidateFiles.length,
      referenceFiles: referenceFiles.length,
      candidateShingles: allCandidate.size,
      referenceShingles: allReference.size,
      exactSharedShingles: shared,
      candidateContainment:
        shared / Math.max(1, allCandidate.size),
      closestPair,
    },
    null,
    2
  )
);
