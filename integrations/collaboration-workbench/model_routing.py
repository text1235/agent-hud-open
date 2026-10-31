"""Choose from the live desktop catalogue without spending exhausted quotas."""
import math
import time

LOW_QUOTA_FRACTION = 0.10
PREFERRED_MODELS = ('gemini-4-argon-high', 'gemini-3.8-flash-high',
                    'gemini-3.8-flash-medium', 'gemini-3.8-flash-low')


def catalogue(configs):
    result = []
    for config in configs:
        quota = config.get('quotaInfo') or {}
        fraction = quota.get('remainingFraction')
        if (isinstance(fraction, bool) or not isinstance(fraction, (int, float))
                or not math.isfinite(fraction) or not 0 <= fraction <= 1):
            fraction = None
        result.append(dict(id=config.get('modelId'), label=config.get('label'),
                           native_model=(config.get('modelOrAlias') or {}).get('model'),
                           remaining_fraction=fraction, reset_time=quota.get('resetTime'),
                           disabled=bool(config.get('isDisabled') or config.get('disabled')
                                         or config.get('isAvailable') is False or quota.get('disabled'))))
    return result


def select_model(configs, requested='auto'):
    models = catalogue(configs)
    eligible = [m for m in models if m['id'] and m['native_model'] and not m['disabled']
                and m['remaining_fraction'] is not None and m['remaining_fraction'] > LOW_QUOTA_FRACTION]
    # An unknown explicit ID is a configuration error, not permission to guess a model name.
    matches = [m for m in models if m['id'] == requested]
    if requested != 'auto' and (len(matches) > 1 or (not matches and requested not in PREFERRED_MODELS)):
        raise ValueError('桌面目录中找不到唯一精确模型 '+requested)
    if not eligible:
        raise ValueError('没有已确认可用且剩余额度高于10%的模型；未发送任务')
    preferred = ((requested,) if requested != 'auto' else ()) + PREFERRED_MODELS
    chosen = next((m for mid in preferred for m in eligible if m['id'] == mid),
                  max(eligible, key=lambda m: m['remaining_fraction']))
    reason = ('automatic_preference' if requested == 'auto' else
              'requested_available' if chosen['id'] == requested else 'requested_low_or_unavailable')
    return dict(chosen, requested=requested, reason=reason,
                low_quota_fraction=LOW_QUOTA_FRACTION, checked_at=time.time())
