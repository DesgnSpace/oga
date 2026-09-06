(class_declaration declaration_kind: "struct" name: (type_identifier) @name) @def.struct

(class_declaration declaration_kind: "class" name: (type_identifier) @name) @def.class

(class_declaration declaration_kind: "actor" name: (type_identifier) @name) @def.class

(class_declaration declaration_kind: "enum" name: (type_identifier) @name) @def.enum

(class_declaration declaration_kind: "extension" name: (user_type) @name) @def.impl

(protocol_declaration name: (type_identifier) @name) @def.trait

(function_declaration name: (simple_identifier) @name) @def.fn

(protocol_function_declaration name: (simple_identifier) @name) @def.method

(init_declaration "init" @name) @def.method

(property_declaration name: (pattern (simple_identifier) @name)) @def.field

(enum_entry name: (simple_identifier) @name) @def.variant

(typealias_declaration name: (type_identifier) @name) @def.type
