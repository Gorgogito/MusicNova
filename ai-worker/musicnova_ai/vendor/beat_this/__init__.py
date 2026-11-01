"""Beat This! (CPJKU, MIT): definición del modelo, commit b95c8ab0c58c2d9fcfd40508ae8dffbc05ac4f5c.

Solo se incluyen `beat_tracker.py` y `roformer.py` sin cambios salvo los imports.
"""


def replace_state_dict_key(state_dict: dict, old: str, new: str):
    """Replaces `old` in all keys of `state_dict` with `new`."""
    keys = list(state_dict.keys())  # take snapshot of the keys
    for key in keys:
        if old in key:
            state_dict[key.replace(old, new)] = state_dict.pop(key)
    return state_dict
