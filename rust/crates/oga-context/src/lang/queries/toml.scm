(table [(bare_key) (dotted_key) (quoted_key)] @name) @def.module

(table_array_element [(bare_key) (dotted_key) (quoted_key)] @name) @def.module

(pair . (_) @name . [(inline_table) (array)]) @def.module

(pair
  .
  (_) @name
  .
  [(string)
   (integer)
   (float)
   (boolean)
   (offset_date_time)
   (local_date_time)
   (local_date)
   (local_time)]) @def.field

(array [(inline_table) (array)] @name @def.module)
