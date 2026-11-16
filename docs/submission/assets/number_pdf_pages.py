#!/usr/bin/env python3
"""Add visible page numbers to a native export without reflowing its contents.

Use pypdf 7.1.0 from pdf-requirements.txt. Retain the original export and verify
text, links and rendered body pixels separately before distributing the result.
"""
import argparse
from pathlib import Path

from pypdf import PdfReader, PdfWriter
from pypdf.generic import ArrayObject, DecodedStreamObject, DictionaryObject, NameObject


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('source', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    if args.source.resolve() == args.destination.resolve() or args.destination.exists():
        parser.error('retain the source and choose a new output path')
    reader = PdfReader(args.source)
    if reader.is_encrypted:
        parser.error('encrypted documents are outside this export helper')
    writer = PdfWriter(clone_from=reader)
    font = DictionaryObject({
        NameObject('/Type'): NameObject('/Font'),
        NameObject('/Subtype'): NameObject('/Type1'),
        NameObject('/BaseFont'): NameObject('/Times-Roman'),
    })
    font_ref = writer._add_object(font)
    for number, page in enumerate(writer.pages, 1):
        if page.rotation != 0:
            parser.error('rotated pages require an explicitly reviewed layout')
        resources = DictionaryObject(page['/Resources'].get_object())
        fonts = DictionaryObject(resources.get('/Font', DictionaryObject()).get_object())
        if '/KTPageNumber' in fonts:
            parser.error('page-number font resource already exists')
        fonts[NameObject('/KTPageNumber')] = font_ref
        resources[NameObject('/Font')] = fonts
        page[NameObject('/Resources')] = resources
        label = str(number)
        # Times-Roman digits have 500-unit advance; 9 pt -> 4.5 pt per digit.
        x = float(page.mediabox.left) + (float(page.mediabox.width) - 4.5 * len(label)) / 2
        y = float(page.mediabox.bottom) + 32
        stamp = DecodedStreamObject()
        stamp.set_data((f'q /Artifact BMC 0.35 g BT /KTPageNumber 9 Tf '
                        f'1 0 0 1 {x:.3f} {y:.3f} Tm ({label}) Tj ET EMC Q\n').encode('ascii'))
        original = page.get('/Contents')
        if isinstance(original, ArrayObject):
            streams = ArrayObject(original)
        else:
            streams = ArrayObject([original]) if original is not None else ArrayObject()
        # Native exports may leave a transformed coordinate system active.
        # Isolate the original page's graphics state before drawing the footer.
        save_state = DecodedStreamObject()
        save_state.set_data(b'q\n')
        restore_state = DecodedStreamObject()
        restore_state.set_data(b'Q\n')
        page[NameObject('/Contents')] = ArrayObject([
            writer._add_object(save_state), *streams,
            writer._add_object(restore_state), writer._add_object(stamp),
        ])
    with args.destination.open('xb') as output:
        writer.write(output)


if __name__ == '__main__':
    main()
