(function_definition name: (identifier) @name) @def.fn

(class_definition name: (identifier) @name) @def.class

(module (expression_statement (assignment left: (identifier) @name) @def.const))

(class_definition
  body: (block (expression_statement (assignment left: (identifier) @name) @def.field)))
