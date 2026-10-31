import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from gateway import Gateway


class DeliveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.g = Gateway(self.root / 'state')
        self.addCleanup(self.temp.cleanup)
        self.addCleanup(self.g.db.close)
        self.root_patch = patch('gateway.ROOT', self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.project = self.g.dispatch('gateway_ensure_project', {'workspace':str(self.root)})
        self.args = dict(project_id=self.project['id'],objective='x',boundary='x',acceptance='x',request_key='root')
        self.task = self.g.dispatch('gateway_submit', self.args)
        self.thread = '01a162e1-7b90-79a0-af60-3f0ea9beb04c'
        (self.root/'review-queues').mkdir()
        self.queue = self.root/'review-queues'/'ours.json'
        self.queue.write_text(json.dumps(dict(project_id=self.project['id'],thread_id=self.thread,roots=[self.task['id']])))
        self.cli = self.root/'codex.exe'
        self.cli.touch()
        self.binding = dict(task_id=self.task['id'],thread_id=self.thread,queue_path=str(self.queue),codex_path=str(self.cli))

    def bind(self):
        return self.g.dispatch('gateway_bind_delivery', self.binding)

    def state(self):
        return self.g.dispatch('gateway_delivery_status', {'task_id':self.task['id']})['state']

    def test_terminal_only_once_and_ack(self):
        self.bind()
        with patch('gateway.subprocess.Popen') as spawn:
            spawn.return_value.communicate.return_value = ('Queued message id', None)
            spawn.return_value.returncode = 0
            spawn.return_value.poll.return_value = 0
            spawn.return_value.pid = 123
            self.assertFalse(self.g.deliver_once())
            self.g.update(self.task['id'],'completed',result='UNTRUSTED_OUTPUT')
            self.assertTrue(self.g.deliver_once())
            self.assertEqual(self.state(),'queued')
            self.assertFalse(self.g.deliver_once())
            self.bind()
            self.assertFalse(self.g.deliver_once())
            spawn.assert_called_once()
            args = spawn.call_args.args[0]
            self.assertEqual(args[1:4],['queue','--thread',self.thread])
            self.assertNotIn('UNTRUSTED_OUTPUT',args[-1])
        self.g.dispatch('gateway_delivery_ack',dict(task_id=self.task['id'],thread_id=self.thread))
        self.assertEqual(self.state(),'consumed')

    def test_invalid_route_and_wrong_chat_ack(self):
        bad = dict(self.binding,thread_id='11111111-1111-1111-1111-111111111111')
        with self.assertRaises(ValueError):
            self.g.dispatch('gateway_bind_delivery',bad)
        self.bind()
        with self.assertRaises(ValueError):
            self.g.dispatch('gateway_delivery_ack',dict(task_id=self.task['id'],thread_id=bad['thread_id']))

    def test_failure_and_restart_do_not_replay(self):
        self.bind()
        self.g.update(self.task['id'],'completed',result='done')
        with patch('gateway.subprocess.Popen',side_effect=FileNotFoundError('missing')) as spawn:
            self.g.deliver_once()
            self.assertEqual(self.state(),'failed')
            self.assertFalse(self.g.deliver_once())
            spawn.assert_called_once()
        self.g.db.execute("UPDATE deliveries SET state='sending'")
        self.g.db.commit()
        other = Gateway(self.root/'state')
        try:
            self.assertEqual(other.dispatch('gateway_delivery_status',dict(task_id=self.task['id']))['state'],'uncertain')
            self.assertFalse(other.deliver_once())
        finally:
            other.db.close()

    def test_repair_inherits_route_without_touching_other_tasks(self):
        self.bind()
        self.g.update(self.task['id'],'completed',result='done')
        child = self.g.dispatch('gateway_submit',dict(self.args,request_key='repair',parent_id=self.task['id']))
        route = self.g.dispatch('gateway_delivery_status',dict(task_id=child['id']))
        self.assertEqual(route['thread'],self.thread)
        self.assertEqual(route['queue'],str(self.queue.resolve()))
        unrelated = self.g.dispatch('gateway_submit',dict(self.args,request_key='unrelated'))
        self.assertIsNone(self.g.dispatch('gateway_delivery_status',dict(task_id=unrelated['id'])))


if __name__ == '__main__':
    unittest.main()
