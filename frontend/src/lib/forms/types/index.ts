/**
 * Field types and the schema parser (contract C2 §4.3): `FIELD_TYPES` is the one descriptor table
 * and `parseSchema` the one schema parser of the client.
 */

export {
	FIELD_TYPES,
	dateTimeText,
	descriptorDrift,
	fieldTypeDescriptor,
	type CelType,
	type FieldTypeDescriptor,
	type NormalizeResult,
	type StorageKind
} from './descriptors.js';
export {
	FORM_SCHEMA_VERSION,
	MODELED_FIELD_KEYS,
	parseSchema,
	unmodeledMembers,
	type FormSchemaV2
} from './schema.js';
