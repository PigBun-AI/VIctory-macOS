"""Offline contract checks: python3 tools/mac/test-matrix-priority.py."""
import csv
from pathlib import Path
import runpy
import unittest

mod = runpy.run_path(str(Path(__file__).with_name('matrix-priority.py')))
rank = mod['rank_rows']


def row(i='a', **kw):
    return dict(dict(id=i, notes='', owner='mac', used='yes', status='gap', freq='unknown',
                     symptom='geom0 color0 flicker0 slow0 crash3', cost='4估', tests='', restore_plan='test'), **kw)


class Priority(unittest.TestCase):
    def test_frequency_cost_tie_and_tags(self):
        a = row('floor', freq='6 of 8', symptom=row()['symptom']+' freq=floor')
        rows = list(rank([a, row('z', cost='0.1'), row('b', cost='0.25')]))
        self.assertEqual([r[1] for r in rows], ['b','z','floor'])
        self.assertEqual(rows[0][6], rows[1][6])
        self.assertEqual(rows[2][3], '10000 (freq=floor)')
        self.assertAlmostEqual(float(rows[2][6]), 3.000033, places=6)
        self.assertIn('unknown', rows[0][3])

    def test_exclusions(self):
        rs = [row('p',status='proven',cost=''),row('unused',used='no'),row('r',notes='仅参照（D3DMetal）'),
              row('s',symptom=row()['symptom']+' summary'),row('c',symptom=row()['symptom']+' closed')]
        rows = list(rank(rs))
        self.assertEqual(len(rows),4)
        self.assertTrue(all(r[0]=='-' and not r[6] for r in rows))

    def test_invalid(self):
        for rs in ([row(cost='?')],[row(symptom='1/0/0/0/0')],[row(status='proven')],[row(),row()]):
            with self.assertRaises(ValueError):
                list(rank(rs))

    def test_entire_matrix_by_header(self):
        with mod['MATRIX'].open(newline='') as f:
            reader = csv.DictReader(f)
            self.assertEqual(reader.fieldnames[-2:],['symptom','cost'])
            rows = list(reader)
        view = list(rank(rows))
        self.assertEqual(len(view),sum(r['status']!='proven' for r in rows))
        self.assertEqual(view,list(rank([dict(reversed(list(r.items()))) for r in reversed(rows)])))


if __name__=='__main__':
    unittest.main()
