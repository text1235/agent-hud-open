import unittest
from model_routing import select_model


def model(mid, fraction, **extra):
    return dict(modelId=mid, label=mid, modelOrAlias={'model':mid+'-native'},
                quotaInfo={'remainingFraction':fraction}, **extra)


class RoutingContracts(unittest.TestCase):
    def test_argon_above_threshold_stays_preferred(self):
        models=[model('gemini-4-argon-high',.2039144),model('gemini-3.8-flash-high',1)]
        self.assertEqual(select_model(models)['id'],'gemini-4-argon-high')

    def test_ten_percent_and_zero_switch_to_flash(self):
        for remaining in (.10,.09,0):
            with self.subTest(remaining=remaining):
                models=[model('gemini-4-argon-high',remaining),model('gemini-3.8-flash-high',1)]
                selected=select_model(models,'gemini-4-argon-high')
                self.assertEqual(selected['id'],'gemini-3.8-flash-high')
                self.assertEqual(selected['requested'],'gemini-4-argon-high')
                self.assertEqual(selected['reason'],'requested_low_or_unavailable')

    def test_shared_argon_pool_does_not_count_as_a_fresh_quota(self):
        models=[model('gemini-4-argon-'+level,.05) for level in ('high','medium','low')]
        models.append(model('gemini-3.8-flash-high',.80))
        self.assertEqual(select_model(models)['id'],'gemini-3.8-flash-high')

    def test_removed_preferred_model_uses_live_flash(self):
        selected=select_model([model('gemini-3.8-flash-high',1)],'gemini-4-argon-high')
        self.assertEqual(selected['id'],'gemini-3.8-flash-high')

    def test_unknown_or_invalid_quota_is_not_assumed_available(self):
        for fraction in (None,True,'1',float('nan'),float('inf'),-1,2):
            with self.subTest(fraction=fraction):
                with self.assertRaises(ValueError):select_model([model('gemini-4-argon-high',fraction)])
        missing=model('gemini-4-argon-high',1);missing['quotaInfo']={'resetTime':'tomorrow'}
        with self.assertRaises(ValueError):select_model([missing])

    def test_disabled_and_missing_mapping_are_excluded(self):
        disabled=model('gemini-4-argon-high',1,isDisabled=True)
        flash=model('gemini-3.8-flash-high',1)
        self.assertEqual(select_model([disabled,flash])['id'],flash['modelId'])
        flash['modelOrAlias']={}
        with self.assertRaises(ValueError):select_model([disabled,flash])

    def test_exact_requested_usable_model_is_respected(self):
        models=[model('gemini-4-argon-high',.5),model('gemini-3.8-flash-high',1)]
        self.assertEqual(select_model(models,'gemini-3.8-flash-high')['id'],'gemini-3.8-flash-high')
        with self.assertRaises(ValueError):select_model(models,'invented-model')

    def test_another_live_model_can_be_used_when_preferred_models_are_exhausted(self):
        models=[model('gemini-4-argon-high',0),model('claude-sonnet-5-5-high',.8)]
        self.assertEqual(select_model(models)['id'],'claude-sonnet-5-5-high')

    def test_all_low_or_exhausted_stops_before_sending(self):
        with self.assertRaises(ValueError):select_model([model('gemini-4-argon-high',.1)])


if __name__=='__main__':unittest.main()
