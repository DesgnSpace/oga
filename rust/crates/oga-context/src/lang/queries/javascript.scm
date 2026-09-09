(function_declaration name: (identifier) @name) @def.fn

(generator_function_declaration name: (identifier) @name) @def.fn

(variable_declarator name: (identifier) @name) @def.const

(class_declaration name: (identifier) @name) @def.class

(method_definition name: (property_identifier) @name) @def.method

(field_definition property: (property_identifier) @name) @def.field
