(function_definition
  declarator: (function_declarator declarator: (identifier) @name)) @def.fn

(declaration
  declarator: (function_declarator declarator: (identifier) @name)) @def.fn

(struct_specifier name: (type_identifier) @name body: (_)) @def.struct

(union_specifier name: (type_identifier) @name body: (_)) @def.struct

(enum_specifier name: (type_identifier) @name) @def.enum

(enumerator name: (identifier) @name) @def.variant

(type_definition declarator: (type_identifier) @name) @def.type

(field_declaration declarator: (field_identifier) @name) @def.field
