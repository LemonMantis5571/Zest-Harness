// Six rules vendored from anti-slop (https://github.com/dmmulroy/anti-slop),
// commit c44ef22ca116, ported from TypeScript to plain ESM with the `create`
// rule API so they load beside zest-boundaries.mjs without a build step or the
// @oxlint/plugins dependency. Rule logic and messages are unchanged; anti-slop
// is meant to be vendored and edited locally, so adjust here, not upstream.
//
// MIT License
//
// Copyright (c) 2026 Dillon Mulroy
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

// ---------------------------------------------------------------------------
// Shared helpers (upstream shared/array-method.ts, shared/scope.ts)

/** Unwrap syntax-only wrappers when inspecting array methods and accumulator references. */
function unwrapArrayExpression(node) {
  while (
    node.type === "ParenthesizedExpression" ||
    node.type === "ChainExpression" ||
    node.type === "TSAsExpression" ||
    node.type === "TSTypeAssertion" ||
    node.type === "TSNonNullExpression" ||
    node.type === "TSSatisfiesExpression"
  ) {
    node = node.expression;
  }
  return node;
}

/** Resolve an identifier to its binding by walking lexical scopes upward. */
function resolveVariable(sourceCode, identifier) {
  let scope = sourceCode.getScope(identifier);
  while (scope !== null) {
    const variable = scope.set.get(identifier.name);
    if (variable !== undefined) return variable;
    scope = scope.upper;
  }
  return null;
}

/** Resolve a local binding by scope, not by identifier spelling. */
function resolveArrayBinding(sourceCode, node) {
  node = unwrapArrayExpression(node);
  if (node.type !== "Identifier") return null;
  return resolveVariable(sourceCode, node);
}

/** Read static method names, including computed string literals, without evaluating expressions. */
function arrayMethodTarget(node) {
  node = unwrapArrayExpression(node);
  if (node.type !== "MemberExpression") return null;
  const property = node.property;
  if (!node.computed && property.type === "Identifier") {
    return { name: property.name, object: node.object };
  }
  if (node.computed && property.type === "Literal" && typeof property.value === "string") {
    return { name: property.value, object: node.object };
  }
  return null;
}

function isArrayAnnotation(type) {
  if (type.type === "TSArrayType" || type.type === "TSTupleType") return true;
  if (type.type === "TSParenthesizedType") return isArrayAnnotation(type.typeAnnotation);
  if (type.type === "TSTypeOperator" && type.operator === "readonly") {
    return isArrayAnnotation(type.typeAnnotation);
  }
  return (
    type.type === "TSTypeReference" &&
    type.typeName.type === "Identifier" &&
    (type.typeName.name === "Array" || type.typeName.name === "ReadonlyArray")
  );
}

/** Recognize local array evidence; unknown receivers and iterator pipelines are deliberately excluded. */
function isKnownArrayExpression(sourceCode, node, visited = new Set()) {
  node = unwrapArrayExpression(node);
  if (node.type === "ArrayExpression") return true;
  if (node.type === "CallExpression") {
    const method = arrayMethodTarget(node.callee);
    return (
      method !== null &&
      ["map", "filter", "flatMap", "slice", "concat", "toSorted", "toReversed", "toSpliced"].includes(method.name) &&
      isKnownArrayExpression(sourceCode, method.object, visited)
    );
  }
  if (node.type !== "Identifier") return false;
  const variable = resolveArrayBinding(sourceCode, node);
  if (variable === null || visited.has(variable)) return false;
  visited.add(variable);
  if (variable.references.some((reference) => reference.isWrite() && !reference.init)) return false;
  for (const identifier of variable.identifiers) {
    const annotation = identifier.typeAnnotation?.typeAnnotation;
    if (annotation !== undefined) return isArrayAnnotation(annotation);
  }
  for (const definition of variable.defs) {
    if (
      definition.type === "Variable" &&
      definition.node.type === "VariableDeclarator" &&
      definition.node.id.type === "Identifier" &&
      definition.node.init !== null &&
      definition.node.parent.type === "VariableDeclaration" &&
      definition.node.parent.kind === "const"
    ) {
      return isKnownArrayExpression(sourceCode, definition.node.init, visited);
    }
  }
  return false;
}

// ---------------------------------------------------------------------------
// no-array-filter-map

/** Reject eager array filter/map pipelines; lazy iterator helpers remain allowed. */
const noArrayFilterMap = {
  meta: {
    type: "suggestion",
    docs: {
      description:
        "Disallow adjacent array filter/map passes in favor of lazy iterator helpers or a single transformation.",
    },
    messages: {
      arrayFilterMap:
        "Avoid consecutive array `{{first}}` and `{{second}}` passes. Prefer `.values().{{first}}(...).{{second}}(...).toArray()` where iterator helpers are supported, or a single `flatMap`/mutating reducer. Preserve callback ordering, indexes, and filtering semantics.",
    },
  },
  create(context) {
    return {
      CallExpression(node) {
        const outer = arrayMethodTarget(node.callee);
        if (outer === null || (outer.name !== "map" && outer.name !== "filter")) return;
        const innerCall = unwrapArrayExpression(outer.object);
        if (innerCall.type !== "CallExpression") return;
        const inner = arrayMethodTarget(innerCall.callee);
        if (inner === null || inner.name !== (outer.name === "map" ? "filter" : "map")) return;
        if (!isKnownArrayExpression(context.sourceCode, inner.object)) return;
        context.report({ node, messageId: "arrayFilterMap", data: { first: inner.name, second: outer.name } });
      },
    };
  },
};

// ---------------------------------------------------------------------------
// no-reduce-accumulator-copy

function enclosingReducer(node) {
  let parent = node.parent;
  while (parent !== null) {
    if (parent.type === "FunctionDeclaration") return null;
    if (parent.type === "ArrowFunctionExpression" || parent.type === "FunctionExpression") {
      const callback = parent;
      let owner = callback.parent;
      while (owner !== null && unwrapArrayExpression(owner) === callback) owner = owner.parent;
      if (owner?.type !== "CallExpression") return null;
      const method = arrayMethodTarget(owner.callee);
      const firstArgument = owner.arguments[0];
      if (
        method === null ||
        (method.name !== "reduce" && method.name !== "reduceRight") ||
        owner.arguments.length > 2 ||
        firstArgument === undefined ||
        unwrapArrayExpression(firstArgument) !== callback
      ) {
        return null;
      }
      const firstParameter = callback.params[0];
      const accumulator = firstParameter?.type === "AssignmentPattern" ? firstParameter.left : firstParameter;
      if (accumulator?.type !== "Identifier") return null;
      return { callback, accumulator, initialValue: owner.arguments[1] };
    }
    parent = parent.parent;
  }
  return null;
}

function referencesAccumulator(sourceCode, node, accumulator, visited = new Set()) {
  const variable = resolveArrayBinding(sourceCode, node);
  if (variable === null || visited.has(variable)) return false;
  if (variable === accumulator) return true;
  visited.add(variable);
  if (variable.references.some((reference) => reference.isWrite() && !reference.init)) return false;
  for (const definition of variable.defs) {
    if (
      definition.type === "Variable" &&
      definition.node.type === "VariableDeclarator" &&
      definition.node.id.type === "Identifier" &&
      definition.node.init !== null &&
      definition.node.parent.type === "VariableDeclaration" &&
      definition.node.parent.kind === "const"
    ) {
      return referencesAccumulator(sourceCode, definition.node.init, accumulator, visited);
    }
  }
  return false;
}

function isGlobalCopyOwner(sourceCode, node, name) {
  node = unwrapArrayExpression(node);
  if (node.type !== "Identifier" || node.name !== name) return false;
  const variable = resolveArrayBinding(sourceCode, node);
  return variable === null || variable.defs.length === 0;
}

/** Reject non-spread copies of reducer accumulators; pair with oxc/no-accumulating-spread. */
const noReduceAccumulatorCopy = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow copying growing reducer accumulators with Object.assign, Array.from, or array copy methods.",
    },
    messages: {
      accumulatorCopy:
        "Do not copy the reducer accumulator on every iteration; growing copies can cause quadratic work. Mutate a fresh, locally owned accumulator and return it, or use an iterator pipeline/flatMap.",
    },
  },
  create(context) {
    return {
      CallExpression(node) {
        const method = arrayMethodTarget(node.callee);
        if (method === null) return;
        const reducer = enclosingReducer(node);
        if (reducer === null) return;
        const accumulator = context.sourceCode
          .getDeclaredVariables(reducer.callback)
          .find((variable) =>
            variable.identifiers.some((identifier) => identifier.start === reducer.accumulator.start),
          );
        if (accumulator === undefined) return;
        const isAccumulator = (expression) => referencesAccumulator(context.sourceCode, expression, accumulator);
        let copiesAccumulator = false;
        if (method.name === "assign" && isGlobalCopyOwner(context.sourceCode, method.object, "Object")) {
          const target = node.arguments[0];
          copiesAccumulator =
            target !== undefined &&
            unwrapArrayExpression(target).type === "ObjectExpression" &&
            node.arguments.slice(1).some(isAccumulator);
        } else if (method.name === "from" && isGlobalCopyOwner(context.sourceCode, method.object, "Array")) {
          const source = node.arguments[0];
          copiesAccumulator = source !== undefined && isAccumulator(source);
        } else if (["concat", "slice", "toSpliced", "toSorted", "toReversed", "with"].includes(method.name)) {
          const initialValue = reducer.initialValue;
          const arrayAccumulator =
            initialValue !== undefined && isKnownArrayExpression(context.sourceCode, initialValue);
          copiesAccumulator = arrayAccumulator && isAccumulator(method.object);
        }
        if (copiesAccumulator) context.report({ node, messageId: "accumulatorCopy" });
      },
    };
  },
};

// ---------------------------------------------------------------------------
// no-chained-type-assertions

function isTypeAssertionExpression(node) {
  return node.type === "TSAsExpression" || node.type === "TSTypeAssertion";
}

function unwrapParenthesizedExpression(expression) {
  let current = expression;
  while (current.type === "ParenthesizedExpression") current = current.expression;
  return current;
}

function isConstAssertion(node) {
  const { typeAnnotation } = node;
  return (
    typeAnnotation.type === "TSTypeReference" &&
    typeAnnotation.typeName.type === "Identifier" &&
    typeAnnotation.typeName.name === "const"
  );
}

function isOutermostAssertionInChain(node) {
  let current = node;
  let parent = node.parent;
  while (parent.type === "ParenthesizedExpression" && parent.expression === current) {
    current = parent;
    parent = parent.parent;
  }
  return !isTypeAssertionExpression(parent) || parent.expression !== current;
}

function isForbiddenAssertionChain(node) {
  let assertionCount = 0;
  let hasNonConstAssertion = false;
  let current = node;
  while (isTypeAssertionExpression(current)) {
    assertionCount += 1;
    hasNonConstAssertion ||= !isConstAssertion(current);
    current = unwrapParenthesizedExpression(current.expression);
  }
  return assertionCount > 1 && hasNonConstAssertion;
}

/** Disallow nested TypeScript type assertions, while permitting chains made only of const assertions. */
const noChainedTypeAssertions = {
  meta: {
    type: "problem",
    docs: {
      description: "Disallow chained TypeScript as and angle-bracket assertions, including parenthesized chains.",
    },
    messages: {
      chained:
        "This assertion chain discards type evidence. Keep the original precise type, or parse untrusted input at its boundary before narrowing it.",
    },
  },
  create(context) {
    const checkTypeAssertion = (node) => {
      if (!isOutermostAssertionInChain(node) || !isForbiddenAssertionChain(node)) return;
      context.report({ node, messageId: "chained" });
    };
    return {
      TSAsExpression: checkTypeAssertion,
      TSTypeAssertion: checkTypeAssertion,
    };
  },
};

// ---------------------------------------------------------------------------
// no-conditional-empty-object-spread

function isEmptyObjectExpression(node) {
  return node.type === "ObjectExpression" && node.properties.length === 0;
}

function isConditionalEmptyObjectSpread(node) {
  const conditional = unwrapParenthesizedExpression(node);
  return (
    conditional.type === "ConditionalExpression" &&
    (isEmptyObjectExpression(conditional.consequent) || isEmptyObjectExpression(conditional.alternate))
  );
}

/** Ban conditional empty-object spreads without changing their omission semantics. */
const noConditionalEmptyObjectSpread = {
  meta: {
    type: "suggestion",
    docs: {
      description: "Disallow object spreads that conditionally spread an empty object to omit fields.",
    },
    messages: {
      avoid:
        "This conditional spread hides property omission behind an empty object. Build the object in separate statements and add the property only when present.",
    },
  },
  create(context) {
    return {
      SpreadElement(node) {
        if (node.parent.type !== "ObjectExpression") return;
        if (isConditionalEmptyObjectSpread(node.argument)) context.report({ node, messageId: "avoid" });
      },
    };
  },
};

// ---------------------------------------------------------------------------
// no-module-mocking

const moduleMockMethods = new Set(["doMock", "mock", "unstable_mockModule"]);

function importedName(node) {
  if (node.type !== "ImportSpecifier") return null;
  return node.imported.type === "Identifier" ? node.imported.name : node.imported.value;
}

function isTestFrameworkObject(sourceCode, expression) {
  if (expression.type !== "Identifier") return false;
  if ((expression.name === "vi" || expression.name === "jest") && sourceCode.isGlobalReference(expression)) {
    return true;
  }
  const variable = resolveVariable(sourceCode, expression);
  if (variable === null || variable.defs.length === 0) {
    return expression.name === "vi" || expression.name === "jest";
  }
  return variable.defs.some((definition) => {
    if (definition.type !== "ImportBinding" || definition.parent?.type !== "ImportDeclaration") return false;
    const source = definition.parent.source.value;
    const name = importedName(definition.node);
    return (source === "vitest" && name === "vi") || (source === "@jest/globals" && name === "jest");
  });
}

function moduleMockCall(sourceCode, callee) {
  if (!("property" in callee) || !("object" in callee) || !("computed" in callee)) return false;
  if (!isTestFrameworkObject(sourceCode, callee.object)) return false;
  const property = callee.property;
  const method = callee.computed
    ? property.type === "Literal" && moduleMockMethods.has(property.value)
      ? property.value
      : null
    : property.type === "Identifier"
      ? property.name
      : null;
  return method !== null && moduleMockMethods.has(method);
}

/** Ban test framework module mocking in favor of real dependency seams. */
const noModuleMocking = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow Vitest and Jest module mocking; tests must replace dependencies through real interfaces.",
    },
    messages: {
      moduleMock:
        "Replace module mocking with dependency injection through a real interface, service layer, or faithful test implementation.",
    },
  },
  create(context) {
    return {
      CallExpression(node) {
        if (node.callee.type === "Super" || node.callee.type === "V8IntrinsicExpression") return;
        if (moduleMockCall(context.sourceCode, node.callee)) context.report({ node, messageId: "moduleMock" });
      },
    };
  },
};

// ---------------------------------------------------------------------------
// no-widen-then-assert

const functionBoundaryTypes = new Set([
  "ArrowFunctionExpression",
  "FunctionDeclaration",
  "FunctionExpression",
  "TSDeclareFunction",
  "TSEmptyBodyFunctionExpression",
]);

function unwrapTypeParentheses(type) {
  let current = type;
  while (current.type === "TSParenthesizedType") current = current.typeAnnotation;
  return current;
}

function typeReferenceName(type) {
  return type.typeName.type === "Identifier" ? type.typeName.name : null;
}

function isUnknownOrAnyType(type) {
  const unwrapped = unwrapTypeParentheses(type);
  return unwrapped.type === "TSUnknownKeyword" || unwrapped.type === "TSAnyKeyword";
}

function isBroadRecordKeyType(type) {
  const unwrapped = unwrapTypeParentheses(type);
  if (
    unwrapped.type === "TSStringKeyword" ||
    unwrapped.type === "TSNumberKeyword" ||
    unwrapped.type === "TSSymbolKeyword"
  ) {
    return true;
  }
  if (unwrapped.type === "TSUnionType") return unwrapped.types.every(isBroadRecordKeyType);
  return unwrapped.type === "TSTypeReference" && typeReferenceName(unwrapped) === "PropertyKey";
}

function isBroadRecordType(type) {
  const unwrapped = unwrapTypeParentheses(type);
  if (unwrapped.type === "TSTypeReference") {
    if (typeReferenceName(unwrapped) === "Readonly") {
      const [inner] = unwrapped.typeArguments?.params ?? [];
      return inner !== undefined && isBroadRecordType(inner);
    }
    if (typeReferenceName(unwrapped) !== "Record") return false;
    const parameters = unwrapped.typeArguments?.params ?? [];
    return (
      parameters.length === 2 &&
      parameters[0] !== undefined &&
      parameters[1] !== undefined &&
      isBroadRecordKeyType(parameters[0]) &&
      isUnknownOrAnyType(parameters[1])
    );
  }
  if (unwrapped.type !== "TSTypeLiteral" || unwrapped.members.length !== 1) return false;
  const [member] = unwrapped.members;
  const [parameter] = member?.type === "TSIndexSignature" ? member.parameters : [];
  return (
    member?.type === "TSIndexSignature" &&
    member.parameters.length === 1 &&
    parameter !== undefined &&
    isBroadRecordKeyType(parameter.typeAnnotation.typeAnnotation) &&
    isUnknownOrAnyType(member.typeAnnotation.typeAnnotation)
  );
}

function broadTypeKind(type) {
  const unwrapped = unwrapTypeParentheses(type);
  if (unwrapped.type === "TSUnknownKeyword" || unwrapped.type === "TSAnyKeyword") return "top";
  if (unwrapped.type === "TSObjectKeyword") return "object";
  return isBroadRecordType(unwrapped) ? "record" : null;
}

function assertedExpression(node) {
  return unwrapParenthesizedExpression(node.expression);
}

function assertionFromExpression(expression) {
  const unwrapped = unwrapParenthesizedExpression(expression);
  return isTypeAssertionExpression(unwrapped) ? unwrapped : null;
}

function normalizedTypeText(sourceText, type) {
  return sourceText.slice(type.start, type.end).replaceAll(/\s+/gu, "");
}

function typesHaveSameSyntax(sourceText, left, right) {
  return (
    left !== null &&
    normalizedTypeText(sourceText, unwrapTypeParentheses(left)) ===
      normalizedTypeText(sourceText, unwrapTypeParentheses(right))
  );
}

function isDefinitelyObjectType(type) {
  const unwrapped = unwrapTypeParentheses(type);
  switch (unwrapped.type) {
    case "TSArrayType":
    case "TSConstructorType":
    case "TSFunctionType":
    case "TSMappedType":
    case "TSObjectKeyword":
    case "TSTupleType":
      return true;
    case "TSTypeLiteral":
      return unwrapped.members.length > 0;
    case "TSIntersectionType":
      return unwrapped.types.every(isDefinitelyObjectType);
    case "TSTypeOperator":
      return unwrapped.operator === "readonly" && isDefinitelyObjectType(unwrapped.typeAnnotation);
    default:
      return false;
  }
}

function isDefinitelyNarrowerRecordType(type) {
  const unwrapped = unwrapTypeParentheses(type);
  if (unwrapped.type === "TSTypeLiteral") {
    return unwrapped.members.some((member) => member.type !== "TSIndexSignature");
  }
  if (unwrapped.type !== "TSTypeReference") return false;
  if (typeReferenceName(unwrapped) === "Readonly") {
    const [inner] = unwrapped.typeArguments?.params ?? [];
    return inner !== undefined && isDefinitelyNarrowerRecordType(inner);
  }
  if (typeReferenceName(unwrapped) !== "Record") return false;
  const parameters = unwrapped.typeArguments?.params ?? [];
  return parameters.length === 2 && parameters[1] !== undefined && !isUnknownOrAnyType(parameters[1]);
}

function functionBoundary(node) {
  let current = node.parent;
  while (current !== null && current.type !== "Program") {
    if (functionBoundaryTypes.has(current.type)) return current;
    current = current.parent;
  }
  return null;
}

function resolvedVariableForIdentifier(scopes, identifier) {
  for (const scope of scopes) {
    const reference = scope.references.find(
      (candidate) => candidate.identifier.start === identifier.start && candidate.identifier.end === identifier.end,
    );
    if (reference !== undefined) return reference.resolved;
  }
  return null;
}

function variableDeclarator(variable) {
  for (const definition of variable.defs) {
    if (definition.type === "Variable" && definition.node.type === "VariableDeclarator") return definition.node;
  }
  return null;
}

function knownValueEvidence(expression, scopes, boundary, visitedVariables) {
  const unwrapped = unwrapParenthesizedExpression(expression);
  if (isTypeAssertionExpression(unwrapped)) {
    if (broadTypeKind(unwrapped.typeAnnotation) !== null) return null;
    return { type: unwrapped.typeAnnotation };
  }
  if (unwrapped.type === "Literal" || unwrapped.type === "TemplateLiteral") return { type: null };
  if (
    unwrapped.type === "ArrayExpression" ||
    unwrapped.type === "ArrowFunctionExpression" ||
    unwrapped.type === "ClassExpression" ||
    unwrapped.type === "FunctionExpression" ||
    unwrapped.type === "NewExpression" ||
    unwrapped.type === "ObjectExpression"
  ) {
    return { type: null };
  }
  if (unwrapped.type !== "Identifier") return null;
  const variable = resolvedVariableForIdentifier(scopes, unwrapped);
  if (variable === null || visitedVariables.has(variable)) return null;

  const annotatedIdentifier = variable.identifiers.find(
    (identifier) => identifier.typeAnnotation !== null && identifier.typeAnnotation !== undefined,
  );
  const annotation = annotatedIdentifier?.typeAnnotation?.typeAnnotation;
  if (annotation !== undefined && annotatedIdentifier !== undefined) {
    if (functionBoundary(annotatedIdentifier) !== boundary || broadTypeKind(annotation) !== null) return null;
    return { type: annotation };
  }

  const declarator = variableDeclarator(variable);
  if (
    declarator === null ||
    declarator.parent.type !== "VariableDeclaration" ||
    declarator.parent.kind !== "const" ||
    declarator.init === null ||
    variable.references.some((reference) => reference.isWrite() && !reference.init) ||
    functionBoundary(declarator) !== boundary
  ) {
    return null;
  }
  return knownValueEvidence(declarator.init, scopes, boundary, new Set([...visitedVariables, variable]));
}

function widenedBinding(variable, scopes) {
  const declarator = variableDeclarator(variable);
  if (
    declarator === null ||
    declarator.parent.type !== "VariableDeclaration" ||
    declarator.parent.kind !== "const" ||
    declarator.id.type !== "Identifier" ||
    declarator.init === null ||
    variable.references.some((reference) => reference.isWrite() && !reference.init)
  ) {
    return null;
  }
  const boundary = functionBoundary(declarator);
  const declaredType = declarator.id.typeAnnotation?.typeAnnotation;
  const initializerAssertion = assertionFromExpression(declarator.init);
  const initializerBroadKind =
    initializerAssertion === null ? null : broadTypeKind(initializerAssertion.typeAnnotation);
  const declaredBroadKind = declaredType === undefined ? null : broadTypeKind(declaredType);
  const broadKind = declaredBroadKind ?? initializerBroadKind;
  if (broadKind === null) return null;

  const originalExpression =
    initializerAssertion !== null && initializerBroadKind !== null
      ? assertedExpression(initializerAssertion)
      : declarator.init;
  const evidence = knownValueEvidence(originalExpression, scopes, boundary, new Set([variable]));
  return evidence === null ? null : { broadKind, evidence, declaredAt: declarator.end, boundary };
}

function assertionIsNarrower(sourceText, broadKind, evidence, assertedType) {
  if (broadTypeKind(assertedType) !== null) return false;
  if (broadKind === "top") return true;
  if (typesHaveSameSyntax(sourceText, evidence.type, assertedType)) return true;
  if (broadKind === "object") return isDefinitelyObjectType(assertedType);
  return isDefinitelyNarrowerRecordType(assertedType);
}

/** Detect immutable local bindings that erase a known type and are later asserted back to a narrower type. */
const noWidenThenAssert = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow local const flows that explicitly widen a known value before asserting the widened binding to a narrower type.",
    },
    messages: {
      widenThenAssert:
        'Binding "{{name}}" discards type evidence and later recreates it with an assertion. Keep the precise type from initialization through use; parse boundary input once.',
    },
  },
  create(context) {
    let scopes = [];
    const checkAssertion = (node) => {
      const expression = assertedExpression(node);
      if (expression.type !== "Identifier") return;
      const variable = resolvedVariableForIdentifier(scopes, expression);
      if (variable === null) return;
      const widened = widenedBinding(variable, scopes);
      if (
        widened === null ||
        node.start <= widened.declaredAt ||
        functionBoundary(node) !== widened.boundary ||
        !assertionIsNarrower(context.sourceCode.text, widened.broadKind, widened.evidence, node.typeAnnotation)
      ) {
        return;
      }
      context.report({ node, messageId: "widenThenAssert", data: { name: expression.name } });
    };
    return {
      Program() {
        scopes = context.sourceCode.scopeManager.scopes;
      },
      TSAsExpression: checkAssertion,
      TSTypeAssertion: checkAssertion,
    };
  },
};

export default {
  meta: { name: "anti-slop" },
  rules: {
    "no-array-filter-map": noArrayFilterMap,
    "no-reduce-accumulator-copy": noReduceAccumulatorCopy,
    "no-chained-type-assertions": noChainedTypeAssertions,
    "no-conditional-empty-object-spread": noConditionalEmptyObjectSpread,
    "no-module-mocking": noModuleMocking,
    "no-widen-then-assert": noWidenThenAssert,
  },
};
