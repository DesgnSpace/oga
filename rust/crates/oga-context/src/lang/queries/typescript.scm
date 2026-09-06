(function_declaration name: (identifier) @name) @def.fn

(generator_function_declaration name: (identifier) @name) @def.fn

(function_signature name: (identifier) @name) @def.fn

(variable_declarator name: (identifier) @name) @def.const

(class_declaration name: (type_identifier) @name) @def.class

(abstract_class_declaration name: (type_identifier) @name) @def.class

(interface_declaration name: (type_identifier) @name) @def.trait

(type_alias_declaration name: (type_identifier) @name) @def.type

(enum_declaration name: (identifier) @name) @def.enum

(enum_body (enum_assignment name: (property_identifier) @name) @def.variant)

(enum_body (property_identifier) @name @def.variant)

(internal_module name: (identifier) @name) @def.module

(method_definition name: (property_identifier) @name) @def.method

(method_signature name: (property_identifier) @name) @def.method

(abstract_method_signature name: (property_identifier) @name) @def.method

(public_field_definition name: (property_identifier) @name) @def.field

(property_signature name: (property_identifier) @name) @def.field
