<?php

declare(strict_types=1);

namespace Acme\Sdk\Tests;

use Acme\Sdk\Client;
use PHPUnit\Framework\TestCase;

final class ClientTest extends TestCase
{
    public function testBuildsRequestUrls(): void
    {
        $request = (new Client('https://api.example.com/'))->request('GET', '/users');

        self::assertSame('https://api.example.com/users', $request->url);
    }
}
