(package_declaration (scoped_identifier) @name) @def.module

(class_declaration name: (identifier) @name) @def.class

(interface_declaration name: (identifier) @name) @def.trait

(annotation_type_declaration name: (identifier) @name) @def.trait

(enum_declaration name: (identifier) @name) @def.enum

(enum_constant name: (identifier) @name) @def.variant

(method_declaration name: (identifier) @name) @def.method

(constructor_declaration name: (identifier) @name) @def.method

(field_declaration declarator: (variable_declarator name: (identifier) @name)) @def.field
