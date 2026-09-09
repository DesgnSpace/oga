(namespace_definition name: (namespace_identifier) @name) @def.module

(class_specifier name: (type_identifier) @name body: (_)) @def.class

(struct_specifier name: (type_identifier) @name body: (_)) @def.struct

(enum_specifier name: (type_identifier) @name) @def.enum

(enumerator name: (identifier) @name) @def.variant

(function_definition
  declarator: (function_declarator declarator: (identifier) @name)) @def.fn

(function_definition
  declarator: (function_declarator declarator: (qualified_identifier name: (identifier) @name))) @def.method

(declaration
  declarator: (function_declarator declarator: (identifier) @name)) @def.fn

(type_definition declarator: (type_identifier) @name) @def.type

(field_declaration declarator: (field_identifier) @name) @def.field
