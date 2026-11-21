"""Checks the gold span, model answer and protocol usage rules independently of a live provider."""

import unittest

from benchmark import accepts, gold, usage_counts
from web import wiki_title


class BenchmarkRules(unittest.TestCase):
    def test_gold_offsets_measure_utf8_bytes_and_return_one_based_lines(self):
        text = '# é\ndef wanted():\n    return 1\n'
        start = len('# é\n'.encode())
        needle = {'path': 'example.py', 'start_byte': start, 'end_byte': len(text.encode()), 'name': 'wanted'}
        self.assertEqual(gold({'content': {'example.py': text}}, needle), ('example.py', 2, 'wanted'))

    def test_model_must_select_the_candidate_and_the_actual_definition_line(self):
        hits = [('uses.py', 1, 40), ('example.py', 1, 40), ('example.py', 9, 15)]
        expected = ('example.py', 10, 'wanted')
        self.assertTrue(accepts('{"candidate":2,"line":10}', hits, expected))
        self.assertFalse(accepts('{"candidate":1,"line":10}', hits, expected))
        self.assertFalse(accepts('{"candidate":2,"line":11}', hits, expected))
        self.assertFalse(accepts('{"candidate":true,"line":10}', hits, expected))
        self.assertFalse(accepts('unstructured answer', hits, expected))

    def test_wikipedia_evidence_normalizes_format_without_matching_unrelated_sites(self):
        self.assertEqual(wiki_title('https://en.m.wikipedia.org/wiki/Charlotte_Bront%C3%AB#Life'),
                         'Charlotte Brontë')
        self.assertIsNone(wiki_title('https://example.com/wiki/Charlotte_Bront%C3%AB'))

    def test_messages_adds_reads_and_writes_but_openai_already_includes_them(self):
        self.assertEqual(usage_counts('messages', {
            'input_tokens': 20, 'cache_read_input_tokens': 100,
            'cache_creation_input_tokens': 80, 'output_tokens': 5,
        }), (200, 100, 80, 5))
        self.assertEqual(usage_counts('chat/completions', {
            'prompt_tokens': 200, 'prompt_tokens_details': {'cached_tokens': 100},
            'completion_tokens': 5,
        }), (200, 100, None, 5))
        self.assertEqual(usage_counts('responses', {
            'input_tokens': 200, 'input_tokens_details': {'cached_tokens': 0},
            'output_tokens': 5,
        }), (200, 0, None, 5))
        self.assertEqual(usage_counts('responses', {'input_tokens': 200, 'output_tokens': 5}),
                         (200, None, None, 5))


if __name__ == '__main__':
    unittest.main()
