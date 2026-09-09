(function_item name: (identifier) @name) @def.fn

(function_signature_item name: (identifier) @name) @def.fn

(struct_item name: (type_identifier) @name) @def.struct

(union_item name: (type_identifier) @name) @def.struct

(field_declaration name: (field_identifier) @name) @def.field

(enum_item name: (type_identifier) @name) @def.enum

(enum_variant name: (identifier) @name) @def.variant

(trait_item name: (type_identifier) @name) @def.trait

(impl_item type: (_) @name) @def.impl

(type_item name: (type_identifier) @name) @def.type

(const_item name: (identifier) @name) @def.const

(static_item name: (identifier) @name) @def.static

(mod_item name: (identifier) @name) @def.module

(macro_definition name: (identifier) @name) @def.macro
