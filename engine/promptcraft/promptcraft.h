#ifndef LM_PROMPTCRAFT_H
#define LM_PROMPTCRAFT_H

#ifdef __cplusplus
extern "C" {
#endif

/* Appends camera-move and style language to a user prompt.
 * camera_id / style_id may be NULL or unknown; unknown ids are ignored.
 * Returns a malloc'd string, or NULL on failure (never throws). Release it with pc_free. */
const char* pc_compose(const char* prompt, const char* camera_id, const char* style_id);

/* Returns {"camera":[{"id","name","group"}],"styles":[{"id","name"}]} as a malloc'd string.
 * NULL on failure (never throws). Release it with pc_free. */
const char* pc_presets_json(void);

/* Frees a string returned by pc_compose or pc_presets_json. NULL is allowed. */
void pc_free(const char* p);

#ifdef __cplusplus
}
#endif

#endif /* LM_PROMPTCRAFT_H */
