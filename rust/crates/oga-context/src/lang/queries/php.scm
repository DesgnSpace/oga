(namespace_definition name: (namespace_name) @name) @def.module

(class_declaration name: (name) @name) @def.class

(interface_declaration name: (name) @name) @def.trait

(trait_declaration name: (name) @name) @def.trait

(enum_declaration name: (name) @name) @def.enum

(enum_case name: (name) @name) @def.variant

(function_definition name: (name) @name) @def.fn

(method_declaration name: (name) @name) @def.method

(const_declaration (const_element (name) @name)) @def.const

(property_declaration (property_element (variable_name (name) @name))) @def.field
