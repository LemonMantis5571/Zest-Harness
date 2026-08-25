const SENSITIVE_NAME = /^(?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|private[_-]?key|password|secret|credential|auth[_-]?token|token)$/i;

const EXPRESSION_WRAPPERS = new Set([
  "AwaitExpression",
  "ChainExpression",
  "JSXExpressionContainer",
  "ParenthesizedExpression",
  "TSAsExpression",
  "TSNonNullExpression",
  "TSTypeAssertion",
  "TypeCastExpression",
]);

function unwrapExpression(node) {
  let current = node;
  while (current && EXPRESSION_WRAPPERS.has(current.type)) {
    current = current.type === "AwaitExpression" ? current.argument : current.expression;
  }
  return current;
}

function isMemberExpression(node) {
  return node?.type === "MemberExpression" || node?.type === "OptionalMemberExpression";
}

function propertyName(node) {
  if (!isMemberExpression(node)) return null;
  if (!node.computed && node.property?.type === "Identifier") return node.property.name;
  if (node.computed && node.property?.type === "Literal" && typeof node.property.value === "string") {
    return node.property.value;
  }
  return null;
}

function isMemberCall(node, objectName, methodName) {
  const call = unwrapExpression(node);
  if (call?.type !== "CallExpression") return false;
  const callee = unwrapExpression(call.callee);
  return (
    isMemberExpression(callee) &&
    propertyName(callee) === methodName &&
    unwrapExpression(callee.object)?.type === "Identifier" &&
    unwrapExpression(callee.object).name === objectName
  );
}

function isGlobalMemberCall(node, globalName, objectName, methodName) {
  const call = unwrapExpression(node);
  if (call?.type !== "CallExpression") return false;
  const callee = unwrapExpression(call.callee);
  const receiver = callee && isMemberExpression(callee) ? unwrapExpression(callee.object) : null;
  return (
    isMemberExpression(callee) &&
    propertyName(callee) === methodName &&
    isMemberExpression(receiver) &&
    propertyName(receiver) === objectName &&
    unwrapExpression(receiver.object)?.type === "Identifier" &&
    unwrapExpression(receiver.object).name === globalName
  );
}

function isJsonParseCall(node) {
  return isMemberCall(node, "JSON", "parse") || isGlobalMemberCall(node, "globalThis", "JSON", "parse");
}

function isResponseJsonCall(node) {
  const call = unwrapExpression(node);
  if (call?.type !== "CallExpression") return false;
  const callee = unwrapExpression(call.callee);
  return isMemberExpression(callee) && propertyName(callee) === "json";
}

function typeName(node) {
  if (node?.type === "Identifier") return node.name;
  if (node?.type === "TSQualifiedName") return typeName(node.right);
  return null;
}

function typeArguments(node) {
  const parameters = node?.typeParameters ?? node?.typeArguments;
  return Array.isArray(parameters) ? parameters : parameters?.params ?? [];
}

function isOpenJsonType(node) {
  if (!node) return false;
  switch (node.type) {
    case "TSUnknownKeyword":
      return true;
    case "TSTypeAnnotation":
      return isOpenJsonType(node.typeAnnotation);
    case "TSParenthesizedType":
      return isOpenJsonType(node.typeAnnotation);
    case "TSTypeOperator":
      return isOpenJsonType(node.typeAnnotation);
    case "TSArrayType":
      return isOpenJsonType(node.elementType);
    case "TSTupleType":
      return node.elementTypes.every(isOpenJsonType);
    case "TSUnionType":
    case "TSIntersectionType":
      return node.types.every(isOpenJsonType);
    case "TSTypeReference": {
      const name = typeName(node.typeName);
      if (name === "JsonValue" || name === "ReadonlyJsonValue" || name === "JsonObject" || name === "JsonArray") {
        return true;
      }
      if (name !== "Record") return false;
      const params = typeArguments(node);
      return params.length === 2 && isOpenJsonType(params[1]);
    }
    default:
      return false;
  }
}

function isJsonBoundary(node) {
  return isJsonParseCall(node) || isResponseJsonCall(node);
}

function reportBoundaryType(context, node, typeNode) {
  if (typeNode && !isOpenJsonType(typeNode)) {
    context.report({ node, messageId: "default" });
  }
}

const noUnvalidatedPersistedJson = {
  meta: {
    type: "problem",
    messages: {
      default: "Keep parsed JSON as unknown until a runtime type guard validates the fields you consume.",
    },
  },
  create(context) {
    const jsonBindings = new Set();

    return {
      TSAsExpression(node) {
        if (
          isJsonBoundary(node.expression) ||
          jsonBindings.has(identifierName(node.expression))
        ) {
          reportBoundaryType(context, node, node.typeAnnotation);
        }
      },
      TSTypeAssertion(node) {
        if (
          isJsonBoundary(node.expression) ||
          jsonBindings.has(identifierName(node.expression))
        ) {
          reportBoundaryType(context, node, node.typeAnnotation);
        }
      },
      VariableDeclarator(node) {
        const directBoundary = isJsonBoundary(node.init);
        const sourceBinding = identifierName(node.init);
        if (directBoundary && node.id?.type === "Identifier") {
          jsonBindings.add(node.id.name);
        }
        if (directBoundary || jsonBindings.has(sourceBinding)) {
          reportBoundaryType(context, node, node.id?.typeAnnotation?.typeAnnotation);
        }
      },
      CallExpression(node) {
        if (isJsonBoundary(node) && typeArguments(node).length > 0) {
          reportBoundaryType(context, node, typeArguments(node)[0]);
        }
      },
    };
  },
};

function isSensitiveText(value) {
  return typeof value === "string" && SENSITIVE_NAME.test(value);
}

function containsSensitive(node, seen = new Set()) {
  if (!node || typeof node !== "object" || seen.has(node)) return false;
  seen.add(node);
  if (node.type === "Identifier") return isSensitiveText(node.name);
  if (node.type === "Literal") return isSensitiveText(node.value);
  if (node.type === "TemplateLiteral") {
    return node.quasis.some((quasi) => isSensitiveText(quasi.value?.raw ?? quasi.value?.cooked)) ||
      node.expressions.some((expression) => containsSensitive(expression, seen));
  }
  for (const [key, value] of Object.entries(node)) {
    if (["parent", "loc", "range", "tokens", "comments", "type"].includes(key)) continue;
    if (Array.isArray(value)) {
      if (value.some((item) => containsSensitive(item, seen))) return true;
    } else if (containsSensitive(value, seen)) {
      return true;
    }
  }
  return false;
}

function isStorageSetItem(node) {
  const call = unwrapExpression(node);
  if (call?.type !== "CallExpression" || call.arguments.length < 2) return false;
  const callee = unwrapExpression(call.callee);
  if (!isMemberExpression(callee) || propertyName(callee) !== "setItem") return false;
  const receiver = unwrapExpression(callee.object);
  return (
    receiver?.type === "Identifier" &&
    (receiver.name === "localStorage" || receiver.name === "sessionStorage")
  ) || (
    isMemberExpression(receiver) &&
    ["localStorage", "sessionStorage"].includes(propertyName(receiver))
  );
}

function isConsoleCall(node) {
  const call = unwrapExpression(node);
  if (call?.type !== "CallExpression") return false;
  const callee = unwrapExpression(call.callee);
  return isMemberExpression(callee) && propertyName(callee) !== null &&
    unwrapExpression(callee.object)?.type === "Identifier" &&
    unwrapExpression(callee.object).name === "console";
}

const noSecretPersistenceOrSink = {
  meta: {
    type: "problem",
    messages: {
      default: "Do not persist or log secret-like values in the UI boundary.",
    },
  },
  create(context) {
    return {
      CallExpression(node) {
        const call = unwrapExpression(node);
        if (isStorageSetItem(call)) {
          if (containsSensitive(call.arguments[0]) || containsSensitive(call.arguments[1])) {
            context.report({ node: call, messageId: "default" });
          }
          return;
        }
        if (isConsoleCall(call) && call.arguments.some((argument) => containsSensitive(argument))) {
          context.report({ node: call, messageId: "default" });
        }
      },
    };
  },
};

function isTrustedHtmlExpression(node) {
  const expression = unwrapExpression(node);
  return (
    expression?.type === "CallExpression" &&
    unwrapExpression(expression.callee)?.type === "Identifier" &&
    unwrapExpression(expression.callee).name === "markTrustedHtml"
  );
}

function htmlExpression(attribute) {
  const value = attribute.value;
  const expression = value?.type === "JSXExpressionContainer" ? value.expression : null;
  if (expression?.type !== "ObjectExpression") return null;
  for (const property of expression.properties) {
    if (property.type !== "Property") continue;
    const key = property.computed ? null : property.key;
    if (key?.type === "Identifier" && key.name === "__html") return property.value;
    if (key?.type === "Literal" && key.value === "__html") return property.value;
  }
  return null;
}

const requireSafeHtmlProvenance = {
  meta: {
    type: "problem",
    messages: {
      default: "Route dangerouslySetInnerHTML through markTrustedHtml(value) so the producer is explicit.",
    },
  },
  create(context) {
    return {
      JSXAttribute(node) {
        if (node.name?.name !== "dangerouslySetInnerHTML") return;
        const html = htmlExpression(node);
        if (!html || !isTrustedHtmlExpression(html)) {
          context.report({ node, messageId: "default" });
        }
      },
    };
  },
};

function isEmptyOrConstantHandler(node) {
  if (node?.type !== "ArrowFunctionExpression" && node?.type !== "FunctionExpression") return false;
  if (node.body?.type === "BlockStatement") return node.body.body.length === 0;
  const body = node.body;
  if (!body) return false;
  if (body.type === "Identifier") return body.name === "undefined";
  if (body.type === "Literal") return true;
  if (body.type === "ObjectExpression" || body.type === "ArrayExpression") return true;
  if (body.type === "TemplateLiteral") return body.expressions.length === 0;
  return body.type === "UnaryExpression" && body.operator === "void";
}

const noUnownedBackgroundRejection = {
  meta: {
    type: "problem",
    messages: {
      default: "Name the intentional background-failure policy instead of swallowing the rejection.",
    },
  },
  create(context) {
    return {
      CallExpression(node) {
        const call = unwrapExpression(node);
        const callee = unwrapExpression(call?.callee);
        if (!isMemberExpression(callee) || propertyName(callee) !== "catch") return;
        if (isEmptyOrConstantHandler(call.arguments[0])) {
          context.report({ node: call.arguments[0], messageId: "default" });
        }
      },
    };
  },
};

function isCreateObjectUrl(node) {
  return (
    isMemberCall(node, "URL", "createObjectURL") ||
    isGlobalMemberCall(node, "globalThis", "URL", "createObjectURL") ||
    isGlobalMemberCall(node, "window", "URL", "createObjectURL")
  );
}

function isRevokeObjectUrl(node) {
  return (
    isMemberCall(node, "URL", "revokeObjectURL") ||
    isGlobalMemberCall(node, "globalThis", "URL", "revokeObjectURL") ||
    isGlobalMemberCall(node, "window", "URL", "revokeObjectURL")
  );
}

function identifierName(node) {
  const expression = unwrapExpression(node);
  return expression?.type === "Identifier" ? expression.name : null;
}

const noObjectUrlLeak = {
  meta: {
    type: "problem",
    messages: {
      default: "Revoke every object URL created by this module when its owner is released.",
    },
  },
  create(context) {
    const ownedUrls = [];
    const bindings = new Map();
    const handledCreates = new Set();

    function latestBinding(name) {
      const entries = bindings.get(name);
      return entries?.[entries.length - 1] ?? null;
    }

    function bind(name, entry) {
      const entries = bindings.get(name) ?? [];
      entries.push(entry);
      bindings.set(name, entries);
    }

    function own(name, node) {
      const entry = { node, revoked: false };
      ownedUrls.push(entry);
      bind(name, entry);
      return entry;
    }

    return {
      VariableDeclarator(node) {
        if (node.id?.type !== "Identifier") return;
        const create = unwrapExpression(node.init);
        if (isCreateObjectUrl(create)) {
          own(node.id.name, create);
          handledCreates.add(create);
          return;
        }
        const source = identifierName(node.init);
        const entry = source ? latestBinding(source) : null;
        if (entry) bind(node.id.name, entry);
      },
      AssignmentExpression(node) {
        if (node.left?.type !== "Identifier") return;
        const create = unwrapExpression(node.right);
        if (isCreateObjectUrl(create)) {
          own(node.left.name, create);
          handledCreates.add(create);
          return;
        }
        const source = identifierName(node.right);
        const entry = source ? latestBinding(source) : null;
        if (entry) bind(node.left.name, entry);
      },
      CallExpression(node) {
        const call = unwrapExpression(node);
        if (isRevokeObjectUrl(call)) {
          const name = identifierName(call.arguments[0]);
          const entry = name ? latestBinding(name) : null;
          if (entry) entry.revoked = true;
          return;
        }
        if (isCreateObjectUrl(call) && !handledCreates.has(call)) {
          context.report({ node: call, messageId: "default" });
        }
      },
      "Program:exit"() {
        for (const entry of ownedUrls) {
          if (!entry.revoked) context.report({ node: entry.node, messageId: "default" });
        }
      },
    };
  },
};

export default {
  meta: { name: "zest" },
  rules: {
    "no-unvalidated-persisted-json": noUnvalidatedPersistedJson,
    "no-secret-persistence-or-sink": noSecretPersistenceOrSink,
    "require-safe-html-provenance": requireSafeHtmlProvenance,
    "no-unowned-background-rejection": noUnownedBackgroundRejection,
    "no-object-url-leak": noObjectUrlLeak,
  },
};
