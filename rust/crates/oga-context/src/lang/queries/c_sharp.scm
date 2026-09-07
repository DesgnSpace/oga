(namespace_declaration name: (_) @name) @def.module

(file_scoped_namespace_declaration name: (_) @name) @def.module

(class_declaration name: (identifier) @name) @def.class

(struct_declaration name: (identifier) @name) @def.struct

(interface_declaration name: (identifier) @name) @def.trait

(enum_declaration name: (identifier) @name) @def.enum

(enum_member_declaration name: (identifier) @name) @def.variant

(delegate_declaration name: (identifier) @name) @def.type

(method_declaration name: (identifier) @name) @def.method

(constructor_declaration name: (identifier) @name) @def.method

(property_declaration name: (identifier) @name) @def.field

(field_declaration
  (variable_declaration
    (variable_declarator name: (identifier) @name))) @def.field
